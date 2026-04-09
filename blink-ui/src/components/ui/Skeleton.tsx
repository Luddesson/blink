import type { CSSProperties } from 'react'

interface SkeletonProps {
  className?: string
  width?: string | number
  height?: string | number
  style?: CSSProperties
}

export function Skeleton({ className = '', width, height, style }: SkeletonProps) {
  return (
    <div
      className={`skeleton ${className}`}
      style={{ width, height, ...style }}
    />
  )
}

/** Block of stacked skeleton lines, simulates text content */
export function SkeletonText({ lines = 3, className = '' }: { lines?: number; className?: string }) {
  return (
    <div className={`flex flex-col gap-1.5 ${className}`}>
      {Array.from({ length: lines }).map((_, i) => (
        <Skeleton
          key={i}
          height={10}
          width={i === lines - 1 ? '65%' : '100%'}
        />
      ))}
    </div>
  )
}
