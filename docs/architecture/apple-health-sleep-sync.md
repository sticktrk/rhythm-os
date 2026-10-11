# Apple Health sleep button sync

On iPhone, open **Settings → Apple Health**, choose the room, named schedule,
or home Sleep/Wake buttons that you use for your own sleep, then enable sync.
Allow Rhythm to write Sleep data in the system authorization sheet. Button
intervals appear in Apple Health as **Time in Bed**. They are reported intent,
not measured sleep or sleep stages.

Sync includes retained activity and runs on app resume, server reconnect, every
five minutes while the app is active, and **Sync now**. The app must be open to
sync; this feature does not request Health background delivery. Unlock the
iPhone if a Health write fails. Denied authorization is visible; allow Sleep
writes in Health and try again.

Consent is local to this phone and binds the signed-in account (or local
session), home, durable server identity, and selected activity scope. Switching
homes pauses that selection. Turn sync off to choose a different scope.
Demo activity and provisional endpoint identities cannot export. Sign-out,
account reset and factory reset erase the preference. Cloud or appliance
backup restoration cannot enable Health sync on another phone. Turning sync
off preserves existing Health samples, which the user can delete in Health.

## Activity contract

`RhythmSleepInterval` consumes the existing server activity shape. It pairs
physical `source.kind = switch` events for one scope:

- Room `sleep_on` starts an interval; `sleep_off` closes it.
- Home `set_mode` / `trigger_transition` with `payload.mode = sleep` starts;
  `day` closes.
- Named schedule actions use `payload.automation_action.schedule_id` and a
  separate `schedule:<id>` scope, even though the activity target is `global`.

App actions, automatic schedule boundaries, ordinary light on/off, failed
actions, unknown modes, future/invalid timestamps and incomplete intervals do
not create samples. Duplicate events are collapsed. Repeated Sleep presses
retain the first start; repeated Wake presses cannot invent another interval.
Intervals longer than 24 hours and conflicting or simultaneous Sleep/Wake
boundaries are rejected conservatively. Retained activity may be incomplete;
the app never fabricates a missing bedtime or wake time.

The source merges the existing authenticated `/api/history` (up to 2,000
retained events) with owner/home/server/target-filtered Supabase activity.
Cloud reads paginate in stable timestamp/event order with a cutoff and a
50,000-event bound. A failed/truncated cloud read contributes no partial pages;
a successful local fallback is visibly partial. Failure differs from an empty
history. Repeated full reads pick up delayed uploads without a fragile local
watermark. The existing database, RLS and retention policy are unchanged.

## Native and privacy contract

The `lighting.rhythm.app/apple_health` channel exposes `authorization`,
`requestAuthorization` and `saveIntervals`. It requests write permission only
for `HKCategoryType.sleepAnalysis`, then checks write authorization separately
from completion of the permission request. It saves `inBed` samples in batches
of at most 100 and acknowledges only the HealthKit save result.

Each sample has a deterministic SHA-256 sync identifier derived from account,
home, server, scope and starting event, and uses the completed interval's end
epoch as its sync version. Replaying a successful or uncertain write uses the
same identifier/version. HealthKit compares these keys when saving matching
objects ([Apple sync identifier documentation](https://developer.apple.com/documentation/healthkit/hkmetadatakeysyncidentifier)).

No Health samples are read into Rhythm. Analytics and support logs include
only operation/outcome/failure categories and opaque journey IDs, without sleep
times, durations, sample counts, activity scope IDs or raw platform errors.
In-flight reads are fenced on consent, scope, account and lifecycle changes.
A native batch already submitted may finish after cancellation.

Before store distribution, enable HealthKit on the publisher's app identifier
and regenerate the matching signing profiles. Verify authorization, revocation,
locked-phone retry, Time in Bed display and repeat-sync deduplication on a
physical iPhone. No appliance or database deployment is required.
