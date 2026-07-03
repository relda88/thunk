import { useState } from 'react'
import { toolLabel } from '../lib/helpers'

type Props = {
  name: string
  status: 'running' | 'done' | 'failed'
  summary: string | null
}

function iconColor(status: Props['status']): string {
  if (status === 'running') return 'text-accent-blue'
  if (status === 'done') return 'text-accent-green'
  return 'text-accent-red'
}

export default function ToolActivityItem({ name, status, summary }: Props) {
  const [expanded, setExpanded] = useState(false)
  const label = toolLabel(name)

  const icon =
    status === 'running' ? '◌' :
    status === 'done' ? '✓' : '✗'

  return (
    <div
      className="self-start max-w-[90%] cursor-pointer select-none"
      onClick={() => summary && setExpanded(e => !e)}
    >
      <div className="rounded px-3 py-1 text-xs bg-surface border-l-2 border-border-accent text-text-muted">
        <div className="text-[10px] text-text-faint uppercase tracking-wide mb-1">TOOL</div>
        <div className="flex items-center gap-2">
          <span className={status === 'running' ? `animate-spin inline-block ${iconColor(status)}` : iconColor(status)}>
            {icon}
          </span>
          <span>{label}</span>
          {summary && <span className="text-border-accent">{expanded ? '▲' : '▼'}</span>}
        </div>
      </div>
      {expanded && summary && (
        <div className="mt-px px-3 py-2 text-xs whitespace-pre-wrap bg-surface-expanded text-text-faint border-l-2 border-border">
          {summary}
        </div>
      )}
    </div>
  )
}
