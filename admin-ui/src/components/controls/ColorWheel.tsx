import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent
} from 'react';

import { hsvToRgb, rgbToCss, rgbToHsv, type Rgb } from './colorMath';

/** HSV color wheel matching the Flutter mood sheet: angle = hue,
    distance from center = saturation. Value (brightness) stays at 1;
    pair with a brightness slider for the third axis. */
export function ColorWheel({
  rgb,
  size = 200,
  disabled,
  onChange,
  onCommit
}: {
  rgb: Rgb;
  size?: number;
  disabled?: boolean;
  onChange?: (rgb: Rgb) => void;
  onCommit: (rgb: Rgb) => void;
}) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [dragRgb, setDragRgb] = useState<Rgb | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const scale = window.devicePixelRatio || 1;
    canvas.width = size * scale;
    canvas.height = size * scale;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    const image = ctx.createImageData(canvas.width, canvas.height);
    const radius = canvas.width / 2;
    for (let y = 0; y < canvas.height; y += 1) {
      for (let x = 0; x < canvas.width; x += 1) {
        const dx = x - radius;
        const dy = y - radius;
        const distance = Math.sqrt(dx * dx + dy * dy);
        const index = (y * canvas.width + x) * 4;
        if (distance > radius) {
          image.data[index + 3] = 0;
          continue;
        }
        const hue = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
        const saturation = Math.min(1, distance / radius);
        const color = hsvToRgb({ h: hue, s: saturation, v: 1 });
        image.data[index] = color.r;
        image.data[index + 1] = color.g;
        image.data[index + 2] = color.b;
        // Soften the outer edge to avoid aliasing.
        image.data[index + 3] =
          distance > radius - scale ? Math.round(255 * (radius - distance)) : 255;
      }
    }
    ctx.putImageData(image, 0, 0);
  }, [size]);

  const shown = dragRgb ?? rgb;
  const hsv = rgbToHsv(shown);
  const angle = (hsv.h * Math.PI) / 180;
  const thumbRadius = (hsv.s * size) / 2;
  const thumbX = size / 2 + Math.cos(angle) * thumbRadius;
  const thumbY = size / 2 + Math.sin(angle) * thumbRadius;

  const rgbFromPointer = useCallback(
    (clientX: number, clientY: number): Rgb => {
      const canvas = canvasRef.current;
      if (!canvas) return shown;
      const rect = canvas.getBoundingClientRect();
      const dx = clientX - rect.left - rect.width / 2;
      const dy = clientY - rect.top - rect.height / 2;
      const hue = ((Math.atan2(dy, dx) * 180) / Math.PI + 360) % 360;
      const saturation = Math.min(
        1,
        Math.sqrt(dx * dx + dy * dy) / (rect.width / 2)
      );
      return hsvToRgb({ h: hue, s: saturation, v: 1 });
    },
    [shown]
  );

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled) return;
    event.currentTarget.setPointerCapture(event.pointerId);
    const next = rgbFromPointer(event.clientX, event.clientY);
    setDragRgb(next);
    onChange?.(next);
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || dragRgb === null) return;
    if (!event.currentTarget.hasPointerCapture(event.pointerId)) return;
    const next = rgbFromPointer(event.clientX, event.clientY);
    setDragRgb(next);
    onChange?.(next);
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    if (disabled || dragRgb === null) return;
    event.currentTarget.releasePointerCapture(event.pointerId);
    const next = rgbFromPointer(event.clientX, event.clientY);
    setDragRgb(null);
    onCommit(next);
  }

  return (
    <div
      className={`colorWheel${disabled ? ' disabled' : ''}`}
      style={{ width: size, height: size }}
      onPointerDown={handlePointerDown}
      onPointerMove={handlePointerMove}
      onPointerUp={handlePointerUp}
    >
      <canvas
        ref={canvasRef}
        style={{ width: size, height: size, borderRadius: '50%' }}
      />
      <div
        className="colorWheelThumb"
        style={{
          left: thumbX,
          top: thumbY,
          background: rgbToCss(shown)
        }}
      />
    </div>
  );
}
