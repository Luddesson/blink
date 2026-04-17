import { cn } from '../../lib/cn'

/**
 * Ambient Midnight Aurora background — radial blobs that drift behind content.
 * Place once at the app root. Pointer-events:none so it never interferes.
 */
export default function AuroraBackground({
  className,
  intensity = 'normal',
}: {
  className?: string
  intensity?: 'subtle' | 'normal' | 'intense'
}) {
  const opacityClass =
    intensity === 'subtle' ? 'opacity-40' :
    intensity === 'intense' ? 'opacity-90' :
    'opacity-70'

  return (
    <div
      aria-hidden="true"
      className={cn(
        'fixed inset-0 -z-10 overflow-hidden pointer-events-none',
        opacityClass,
        className,
      )}
    >
      <div className="aurora-bg" />
      {/* Vignette */}
      <div
        className="absolute inset-0"
        style={{
          background:
            'radial-gradient(ellipse at center, transparent 0%, oklch(0.13 0.012 260 / 0.4) 70%, oklch(0.13 0.012 260 / 0.9) 100%)',
        }}
      />
      {/* Fine grain noise for premium depth */}
      <div
        className="absolute inset-0 opacity-[0.035] mix-blend-overlay"
        style={{
          backgroundImage:
            "url(\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='120' height='120'><filter id='n'><feTurbulence type='fractalNoise' baseFrequency='0.9' numOctaves='2' stitchTiles='stitch'/></filter><rect width='120' height='120' filter='url(%23n)'/></svg>\")",
        }}
      />
    </div>
  )
}
