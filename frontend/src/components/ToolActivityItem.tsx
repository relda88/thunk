import { useState } from 'react'
import { toolLabel } from '../lib/helpers'

type Props = {
  name: string
  status: 'running' | 'done' | 'failed'
  summary: string | null
}

export default function ToolActivityItem({ name, status, summary }: Props) {
  const [expanded, setExpanded] = useState(false)
  const label = toolLabel(name)

  const icon =
    status === 'running' ? '◌' :
    status === 'done' ? '✓' : '✗'

  const iconColor =
    status === 'running' ? '#569cd6' :
    status === 'done' ? '#4ec94e' : '#f44444'

  return (
    <div
      className="self-start max-w-[90%] cursor-pointer select-none"
      onClick={() => summary && setExpanded(e => !e)}
    >
      <div
        className="flex items-center gap-2 rounded px-3 py-1 text-xs"
        style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
      >
        <span
          className={status === 'running' ? 'animate-spin inline-block' : ''}
          style={{ color: iconColor }}
        >
          {icon}
        </span>
        <span>{label}</span>
        {summary && <span style={{ color: '#555' }}>{expanded ? '▲' : '▼'}</span>}
      </div>
      {expanded && summary && (
        <div
          className="mt-px px-3 py-2 text-xs whitespace-pre-wrap"
          style={{ background: '#1a1a1a', color: '#666', borderLeft: '2px solid #333' }}
        >
          {summary}
        </div>
      )}
    </div>
  )
}
