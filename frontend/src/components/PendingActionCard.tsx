import { toolLabel } from '../lib/helpers'
import type { PendingActionDto } from '../lib/types'

type Props = {
  pending: PendingActionDto
  index?: number
}

const riskColor: Record<string, string> = {
  low: '#4ec94e',
  medium: '#e5c07b',
  high: '#f44444',
}

export default function PendingActionCard({ pending, index }: Props) {
  const label = toolLabel(pending.tool_name)
  const color = riskColor[pending.risk] ?? '#888'

  return (
    <div>
      <div className="text-sm mb-1">
        {index !== undefined
          ? <><span style={{ color: '#888' }}>{index + 1}. tool: </span>{label}</>
          : <><span style={{ color: '#888' }}>tool: </span>{label}</>
        }
      </div>
      <div className="text-sm mb-1">
        <span style={{ color: '#888' }}>summary: </span>{pending.summary}
      </div>
      <div className="text-sm mb-1">
        <span style={{ color: '#888' }}>risk: </span>
        <span style={{ color }}>{pending.risk}</span>
      </div>
      <div className="text-sm mb-1">
        <span style={{ color: '#888' }}>reversible: </span>
        {pending.reversible ? 'yes' : 'no'}
      </div>
      {!pending.reversible && (
        <div
          className="rounded px-2 py-1 text-sm mb-2"
          style={{ background: '#3a1a1a', border: '1px solid #f44444', color: '#f99999' }}
        >
          &#9888; This action cannot be undone
        </div>
      )}
      {pending.preview.length > 0 && (
        <div
          className="rounded p-2 text-xs whitespace-pre-wrap max-h-48 overflow-y-auto mb-1"
          style={{ background: '#1e1e1e', border: '1px solid #333' }}
        >
          {pending.preview.join('\n')}
        </div>
      )}
    </div>
  )
}
