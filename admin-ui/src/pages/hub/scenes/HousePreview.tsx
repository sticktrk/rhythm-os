import { useMemo, useState } from 'react';
import { Home, Layers, Pin, Shuffle, SlidersHorizontal } from 'lucide-react';

import { rgbToCss } from '../../../components/controls/colorMath';
import { SelectField } from '../../../components/controls/fields';
import { SegmentedControl } from '../../../components/controls/SegmentedControl';
import { Slider } from '../../../components/controls/Slider';
import {
  outputRgb,
  renderScenePreview,
  type HouseOrder,
  type PreviewLight,
  type PreviewScope
} from './scenePalette';

type ScopeKind = PreviewScope['kind'];

/** What every light in the house would receive from the draft, computed the
    way the server deals the palette, before anything is sent to a bulb. */
export function HousePreview({
  scene,
  house,
  loading,
  onRandomize
}: {
  scene: Record<string, unknown>;
  house: HouseOrder | null;
  loading?: boolean;
  /** Deal the palette across the house in a fresh random order (switching
      the layer to shuffle mode); absent when the preview is read-only. */
  onRandomize?: () => void;
}) {
  const hasHouse = house !== null && house.ordered.length > 0;
  const [scopeKind, setScopeKind] = useState<ScopeKind>(hasHouse ? 'home' : 'strip');
  const [roomId, setRoomId] = useState('');
  const [stripCount, setStripCount] = useState(12);
  const [hovered, setHovered] = useState<PreviewLight | null>(null);

  const effectiveKind: ScopeKind = hasHouse ? scopeKind : 'strip';
  const firstRoomId = house?.rooms[0]?.id ?? '';

  const preview = useMemo(() => {
    const scope: PreviewScope =
      effectiveKind === 'home'
        ? { kind: 'home' }
        : effectiveKind === 'room'
          ? { kind: 'room', roomId: roomId || firstRoomId }
          : { kind: 'strip', count: stripCount };
    return renderScenePreview(
      scene,
      house ?? { rooms: [], roomless: [], ordered: [] },
      scope
    );
  }, [scene, house, effectiveKind, roomId, firstRoomId, stripCount]);

  const groups = useMemo(() => {
    const byRoom = new Map<string, { name: string; lights: PreviewLight[] }>();
    for (const light of preview.lights) {
      const key = light.roomId ?? '';
      const name =
        light.roomName ?? (effectiveKind === 'strip' ? 'Lights' : 'No room');
      const group = byRoom.get(key) ?? { name, lights: [] };
      group.lights.push(light);
      byRoom.set(key, group);
    }
    return [...byRoom.values()];
  }, [preview, effectiveKind]);

  const distinct = new Set(
    preview.lights
      .filter((light) => light.output && !light.skipped)
      .map((light) => {
        const rgb = outputRgb(light.output!);
        return `${rgb.r},${rgb.g},${rgb.b}`;
      })
  ).size;
  const active = preview.lights.filter((light) => !light.skipped).length;

  return (
    <div className="ssPreview">
      <div className="ssPreviewControls">
        <SegmentedControl
          value={effectiveKind}
          onChange={(value) => setScopeKind(value as ScopeKind)}
          disabled={!hasHouse}
          options={[
            { value: 'home', label: 'Whole home', icon: <Home size={14} /> },
            { value: 'room', label: 'One room', icon: <Layers size={14} /> },
            {
              value: 'strip',
              label: 'Any N lights',
              icon: <SlidersHorizontal size={14} />
            }
          ]}
        />
        {effectiveKind === 'room' && house ? (
          <SelectField
            value={roomId || house.rooms[0]?.id || ''}
            onChange={setRoomId}
            options={house.rooms.map((room) => ({
              value: room.id,
              label: `${room.name} · ${room.lights.length} light${
                room.lights.length === 1 ? '' : 's'
              }`
            }))}
          />
        ) : null}
        {onRandomize && preview.anchorCount > 0 ? (
          <button
            className="consoleButton small ssRandomize"
            type="button"
            onClick={onRandomize}
            title={
              preview.mode === 'shuffle'
                ? 'Deal the colours across the house again'
                : 'Deal the spread colours across the whole house in random order'
            }
          >
            <Shuffle size={14} />
            {preview.mode === 'shuffle' ? 'Re-roll house' : 'Randomize house'}
          </button>
        ) : null}
        {effectiveKind === 'strip' ? (
          <div className="ssStripSlider">
            <Slider
              value={stripCount}
              min={1}
              max={48}
              label="Lights"
              format={(value) => `${Math.round(value)}`}
              onChange={(value) => setStripCount(Math.round(value))}
              onCommit={(value) => setStripCount(Math.round(value))}
            />
          </div>
        ) : null}
      </div>

      <div className="ssPreviewSummary">
        {loading ? (
          <span>Reading the house…</span>
        ) : (
          <>
            <strong>{active}</strong> light{active === 1 ? '' : 's'} ·{' '}
            <strong>{distinct}</strong> distinct colour{distinct === 1 ? '' : 's'}
            {preview.anchorCount > 0 ? (
              <>
                {' '}
                · {preview.anchorCount} anchor{preview.anchorCount === 1 ? '' : 's'}{' '}
                {preview.mode === 'spread'
                  ? 'spread'
                  : preview.mode === 'shuffle'
                    ? 'shuffled'
                    : 'cycled'}{' '}
                over{' '}
                {preview.span} slot{preview.span === 1 ? '' : 's'}
              </>
            ) : null}
            {!hasHouse && !loading ? (
              <span className="ssPreviewHint">
                {' '}
                · no lights reported yet, showing a generic strip
              </span>
            ) : null}
          </>
        )}
      </div>

      <div className="ssHouse">
        {groups.map((group, index) => (
          <div className="ssRoom" key={`${group.name}-${index}`}>
            <div className="ssRoomName">{group.name}</div>
            <div className="ssBulbs">
              {group.lights.map((light) => (
                <Bulb
                  key={light.id}
                  light={light}
                  hovered={hovered?.id === light.id}
                  onHover={setHovered}
                />
              ))}
            </div>
          </div>
        ))}
        {groups.length === 0 ? (
          <p className="cardNote">Nothing to preview for this scope.</p>
        ) : null}
      </div>

      <div className="ssLegend" aria-live="polite">
        {hovered && hovered.output ? (
          <>
            <span
              className="p5Swatch"
              style={{ background: rgbToCss(outputRgb(hovered.output)) }}
            />
            <strong>{hovered.name}</strong>
            {hovered.pinned ? <span className="ssTag">pinned</span> : null}
            {hovered.slot !== null ? (
              <span className="ssTag">slot {hovered.slot + 1}</span>
            ) : null}
            <span>
              {hovered.output.on
                ? `${hovered.output.brightness}% · ${describeColor(hovered.output)}`
                : 'off'}
            </span>
          </>
        ) : hovered?.skipped ? (
          <>
            <strong>{hovered.name}</strong>
            <span className="ssTag muted">disabled, skipped</span>
          </>
        ) : (
          <span className="ssLegendHint">
            Hover a light to see what it receives. Pinned lights keep their own
            entry; disabled lights are skipped.
          </span>
        )}
      </div>
    </div>
  );
}

function Bulb({
  light,
  hovered,
  onHover
}: {
  light: PreviewLight;
  hovered: boolean;
  onHover: (light: PreviewLight | null) => void;
}) {
  const output = light.output;
  const lit = output?.on ?? false;
  const rgb = output ? outputRgb(output) : { r: 120, g: 120, b: 120 };
  const glow = lit ? 0.35 + (output!.brightness / 100) * 0.65 : 0;
  return (
    <button
      type="button"
      className={`ssBulb${light.skipped ? ' skipped' : ''}${lit ? '' : ' off'}${
        hovered ? ' hovered' : ''
      }`}
      style={
        light.skipped
          ? undefined
          : {
              background: lit ? rgbToCss(rgb) : 'var(--surface-muted)',
              boxShadow: lit
                ? `0 0 ${8 + glow * 14}px ${rgbToCss(rgb)}`
                : undefined,
              opacity: lit ? 0.55 + glow * 0.45 : 0.6
            }
      }
      onMouseEnter={() => onHover(light)}
      onFocus={() => onHover(light)}
      onMouseLeave={() => onHover(null)}
      onBlur={() => onHover(null)}
      aria-label={light.name}
    >
      {light.pinned ? <Pin size={10} /> : null}
    </button>
  );
}

function describeColor(output: { kelvin?: number; rgb?: { r: number; g: number; b: number } }): string {
  if (output.rgb) return `rgb ${output.rgb.r} ${output.rgb.g} ${output.rgb.b}`;
  if (output.kelvin !== undefined) return `${output.kelvin}K`;
  return 'default colour';
}
