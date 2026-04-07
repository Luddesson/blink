interface Props {
  data: number[]
  width?: number
  height?: number
  color?: string
  fillOpacity?: number
  className?: string
}

export default function Sparkline({
  data,
  width = 120,
  height = 32,
  color = '#a6e22e',
  fillOpacity = 0.1,
  className,
}: Props) {
  if (data.length === 0) {
    return <svg width={width} height={height} className={className} />
  }

  if (data.length === 1) {
    return (
      <svg width={width} height={height} className={className}>
        <circle cx={width / 2} cy={height / 2} r={1.5} fill={color} />
      </svg>
    )
  }

  const min = Math.min(...data)
  const max = Math.max(...data)
  const range = max - min || 1

  const pad = 1
  const points = data.map((v, i) => {
    const x = (i / (data.length - 1)) * width
    const y = pad + (1 - (v - min) / range) * (height - pad * 2)
    return `${x},${y}`
  })

  const polyline = points.join(' ')
  const fillPath = `M0,${height} L${points.join(' L')} L${width},${height} Z`

  return (
    <svg width={width} height={height} className={className} viewBox={`0 0 ${width} ${height}`}>
      <path d={fillPath} fill={color} opacity={fillOpacity} />
      <polyline
        points={polyline}
        fill="none"
        stroke={color}
        strokeWidth={1.5}
        strokeLinejoin="round"
        strokeLinecap="round"
      />
    </svg>
  )
}
