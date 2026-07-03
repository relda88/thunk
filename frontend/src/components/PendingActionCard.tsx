import { toolLabel } from '../lib/helpers'
import type { PendingActionDto } from '../lib/types'

type Props = {
  pending: PendingActionDto
  index?: number
}

const riskBadgeClass: Record<string, string> = {
  low: 'bg-accent-green/15 text-accent-green',
  medium: 'bg-accent-yellow/15 text-accent-yellow',
  high: 'bg-accent-red/15 text-accent-red',
}

export default function PendingActionCard({ pending, index }: Props) {
  const label = toolLabel(pending.tool_name)
  const badgeClass = riskBadgeClass[pending.risk] ?? 'bg-surface-raised text-text-muted'

  return (
    <div>
      <div className="text-sm mb-1">
        {index !== undefined
          ? <><span className="text-text-muted">{index + 1}. tool: </span>{label}</>
          : <><span className="text-text-muted">tool: </span>{label}</>
        }
      </div>
      <div className="text-sm mb-1">
        <span className="text-text-muted">summary: </span>{pending.summary}
      </div>
      <div className="text-sm mb-1 flex items-center gap-2">
        <span className="text-text-muted">risk: </span>
        <span className={`rounded-full px-2 py-0.5 text-xs font-medium ${badgeClass}`}>{pending.risk}</span>
      </div>
      <div className="text-sm mb-1">
        <span className="text-text-muted">reversible: </span>
        {pending.reversible ? 'yes' : 'no'}
      </div>
      {!pending.reversible && (
        <div className="rounded px-2 py-1 text-sm mb-2 bg-irreversible-bg border border-accent-red text-red-soft">
          &#9888; This action cannot be undone
        </div>
      )}
      {pending.preview.length > 0 && (
        <div className="rounded p-2 text-xs whitespace-pre-wrap max-h-48 overflow-y-auto mb-1 bg-surface border border-border">
          {pending.preview.join('\n')}
        </div>
      )}
    </div>
  )
}
