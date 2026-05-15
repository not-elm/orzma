import { useEffect, useRef } from 'react';

interface Props {
  /** Latest JPEG payload from `BrowserServerMsg::Screencast`. */
  jpeg: Uint8Array | null;
  /** Canvas backing width (device pixels) — from the same screencast message. */
  width: number;
  /** Canvas backing height (device pixels). */
  height: number;
  /** Optional Tailwind/extra class names. */
  className?: string;
}

/**
 * Renders a screencast JPEG into a `<canvas>` using `createImageBitmap` +
 * `transferFromImageBitmap` for zero-copy painting. Backing size is the
 * frame's device-pixel size; CSS size comes from the parent (e.g. an
 * absolute-inset overlay), so the browser handles downscale.
 *
 * Falls back to `drawImage` on a 2D context if the platform lacks the
 * `bitmaprenderer` context.
 */
export function CanvasFrame({ jpeg, width, height, className }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !jpeg || width <= 0 || height <= 0) return;

    // NOTE: assigning canvas.width/height clears the canvas — required when
    // dimensions change between frames.
    if (canvas.width !== width) canvas.width = width;
    if (canvas.height !== height) canvas.height = height;

    const blob = new Blob([jpeg as Uint8Array<ArrayBuffer>], { type: 'image/jpeg' });
    let cancelled = false;
    createImageBitmap(blob)
      .then((bitmap) => {
        if (cancelled) {
          bitmap.close();
          return;
        }
        const ctx = canvas.getContext('bitmaprenderer') as ImageBitmapRenderingContext | null;
        if (ctx) {
          ctx.transferFromImageBitmap(bitmap);
        } else {
          const fallback = canvas.getContext('2d');
          if (fallback) {
            fallback.drawImage(bitmap, 0, 0);
            bitmap.close();
          }
        }
      })
      .catch(() => {
        // NOTE: malformed JPEG or browser refusal — skip this frame.
      });

    return () => {
      cancelled = true;
    };
  }, [jpeg, width, height]);

  return <canvas ref={canvasRef} className={className} />;
}
