import { useState, useMemo } from 'react'

interface Column<T> {
  key: string
  label: string
  render: (row: T) => React.ReactNode
  sortKey?: (row: T) => number | string
  align?: 'left' | 'right' | 'center'
  width?: string
}

interface Props<T> {
  columns: Column<T>[]
  data: T[]
  keyFn: (row: T) => string
  defaultSort?: string
  defaultDir?: 'asc' | 'desc'
  emptyMessage?: string
  maxHeight?: string
  className?: string
}

export default function SortableTable<T>({
  columns,
  data,
  keyFn,
  defaultSort,
  defaultDir = 'asc',
  emptyMessage = 'No data',
  maxHeight,
  className,
}: Props<T>) {
  const [sortKey, setSortKey] = useState(defaultSort ?? '')
  const [sortDir, setSortDir] = useState<'asc' | 'desc'>(defaultDir)

  function toggleSort(key: string) {
    if (sortKey === key) {
      setSortDir((d) => (d === 'asc' ? 'desc' : 'asc'))
    } else {
      setSortKey(key)
      setSortDir('asc')
    }
  }

  const sorted = useMemo(() => {
    const col = columns.find((c) => c.key === sortKey)
    if (!col?.sortKey) return data

    return [...data].sort((a, b) => {
      const va = col.sortKey!(a)
      const vb = col.sortKey!(b)
      const cmp = typeof va === 'string' ? va.localeCompare(vb as string) : (va as number) - (vb as number)
      return sortDir === 'asc' ? cmp : -cmp
    })
  }, [data, columns, sortKey, sortDir])

  const alignClass = (a?: string) =>
    a === 'right' ? 'text-right' : a === 'center' ? 'text-center' : 'text-left'

  return (
    <div
      className={`overflow-auto ${className ?? ''}`}
      style={maxHeight ? { maxHeight } : undefined}
    >
      <table className="w-full text-[11px] font-mono border-collapse">
        <thead className={maxHeight ? 'sticky top-0 z-10' : undefined}>
          <tr className="border-b border-slate-800 bg-slate-900">
            {columns.map((col) => (
              <th
                key={col.key}
                className={`px-2 py-1.5 font-normal text-slate-500 select-none ${alignClass(col.align)} ${
                  col.sortKey ? 'cursor-pointer hover:text-slate-300' : ''
                }`}
                style={col.width ? { width: col.width } : undefined}
                onClick={() => col.sortKey && toggleSort(col.key)}
              >
                {col.label}
                {col.sortKey && sortKey === col.key && (
                  <span className="ml-1">{sortDir === 'asc' ? '▲' : '▼'}</span>
                )}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {sorted.length === 0 ? (
            <tr>
              <td
                colSpan={columns.length}
                className="text-center text-slate-600 py-6"
              >
                {emptyMessage}
              </td>
            </tr>
          ) : (
            sorted.map((row) => (
              <tr
                key={keyFn(row)}
                className="border-b border-slate-800/50 hover:bg-slate-800/50 transition-colors"
              >
                {columns.map((col) => (
                  <td
                    key={col.key}
                    className={`px-2 py-1.5 text-slate-300 ${alignClass(col.align)}`}
                    style={col.width ? { width: col.width } : undefined}
                  >
                    {col.render(row)}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  )
}
