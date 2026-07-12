//! Transactional Hue room membership changes.

use anyhow::Result;

use crate::transport::HueTransport;

#[derive(Clone, Debug, PartialEq, Eq)]
struct HueRoomMembership {
    room_id: String,
    device_ids: Vec<String>,
}

/// Exact bridge state needed to undo a successful room membership change.
#[derive(Debug)]
pub struct HueRoomAssignmentRollback {
    original: Vec<HueRoomMembership>,
    updated_room_ids: Vec<String>,
}

impl HueRoomAssignmentRollback {
    /// Restore every room changed by the corresponding assignment.
    pub fn rollback<H: HueTransport>(self, transport: &H, username: &str) -> Result<()> {
        rollback_room_updates(transport, username, &self.original, &self.updated_room_ids)
    }
}

fn fetch_room_memberships<H: HueTransport>(
    transport: &H,
    username: &str,
) -> Result<Vec<HueRoomMembership>> {
    let response = transport.get_resources(username, "room")?;
    let rooms = response
        .get("data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| anyhow::anyhow!("Hue room response has no data array"))?;

    rooms
        .iter()
        .map(|room| {
            let room_id = room
                .get("id")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("Hue room response contains a room without an id"))?
                .to_string();
            let mut device_ids = room
                .get("children")
                .and_then(|value| value.as_array())
                .into_iter()
                .flatten()
                .filter(|child| {
                    child.get("rtype").and_then(|value| value.as_str()) == Some("device")
                })
                .filter_map(|child| {
                    child
                        .get("rid")
                        .and_then(|value| value.as_str())
                        .map(str::to_string)
                })
                .collect::<Vec<_>>();
            device_ids.sort();
            device_ids.dedup();
            Ok(HueRoomMembership {
                room_id,
                device_ids,
            })
        })
        .collect()
}

fn membership_matches(
    rooms: &[HueRoomMembership],
    native_device_id: &str,
    target_hub_room_id: Option<&str>,
) -> bool {
    let memberships = rooms
        .iter()
        .filter(|room| {
            room.device_ids
                .iter()
                .any(|device_id| device_id == native_device_id)
        })
        .map(|room| room.room_id.as_str())
        .collect::<Vec<_>>();
    match target_hub_room_id {
        Some(target) => memberships == vec![target],
        None => memberships.is_empty(),
    }
}

fn rollback_room_updates<H: HueTransport>(
    transport: &H,
    username: &str,
    original: &[HueRoomMembership],
    updated_room_ids: &[String],
) -> Result<()> {
    let mut failures = Vec::new();
    for room_id in updated_room_ids.iter().rev() {
        let Some(room) = original.iter().find(|room| &room.room_id == room_id) else {
            continue;
        };
        if let Err(error) = transport.update_room_children(username, room_id, &room.device_ids) {
            log::error!(
                target: "hue_room_membership",
                "Failed to roll back Hue room {} after membership update failure: {}",
                room_id,
                error
            );
            failures.push(format!("{room_id}: {error:#}"));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "Failed to restore Hue room membership: {}",
            failures.join("; ")
        )
    }
}

/// Move a Hue device to exactly one room, or remove it from all Hue rooms.
///
/// The bridge is updated and then re-read before success is returned. Any
/// partial update is rolled back best-effort, allowing the caller to leave its
/// local topology untouched on failure.
pub fn reassign_device_room<H: HueTransport>(
    transport: &H,
    username: &str,
    native_device_id: &str,
    target_hub_room_id: Option<&str>,
) -> Result<HueRoomAssignmentRollback> {
    let original = fetch_room_memberships(transport, username)?;
    if let Some(target) = target_hub_room_id {
        if !original.iter().any(|room| room.room_id == target) {
            return Err(anyhow::anyhow!("Hue target room not found: {}", target));
        }
    }
    if membership_matches(&original, native_device_id, target_hub_room_id) {
        return Ok(HueRoomAssignmentRollback {
            original,
            updated_room_ids: Vec::new(),
        });
    }

    let mut desired = original.clone();
    for room in &mut desired {
        room.device_ids
            .retain(|device_id| device_id != native_device_id);
        if target_hub_room_id == Some(room.room_id.as_str()) {
            room.device_ids.push(native_device_id.to_string());
            room.device_ids.sort();
            room.device_ids.dedup();
        }
    }

    let mut changed = desired
        .iter()
        .filter(|room| {
            original
                .iter()
                .find(|original| original.room_id == room.room_id)
                .is_some_and(|original| original.device_ids != room.device_ids)
        })
        .cloned()
        .collect::<Vec<_>>();
    changed.sort_by_key(|room| {
        if target_hub_room_id == Some(room.room_id.as_str()) {
            1
        } else {
            0
        }
    });

    let mut updated_room_ids = Vec::new();
    for room in &changed {
        if let Err(error) =
            transport.update_room_children(username, &room.room_id, &room.device_ids)
        {
            let _ = rollback_room_updates(transport, username, &original, &updated_room_ids);
            return Err(error.context(format!(
                "Failed to update Hue room {} while moving device {}",
                room.room_id, native_device_id
            )));
        }
        updated_room_ids.push(room.room_id.clone());
    }

    let verified = fetch_room_memberships(transport, username);
    match verified {
        Ok(rooms) if membership_matches(&rooms, native_device_id, target_hub_room_id) => {
            Ok(HueRoomAssignmentRollback {
                original,
                updated_room_ids,
            })
        }
        Ok(_) => {
            let _ = rollback_room_updates(transport, username, &original, &updated_room_ids);
            Err(anyhow::anyhow!(
                "Hue bridge did not persist the requested room membership for device {}",
                native_device_id
            ))
        }
        Err(error) => {
            let _ = rollback_room_updates(transport, username, &original, &updated_room_ids);
            Err(error.context(format!(
                "Failed to verify Hue room membership for device {}",
                native_device_id
            )))
        }
    }
}
