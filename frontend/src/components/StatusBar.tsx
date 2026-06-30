import { activityLabel } from '../lib/helpers'
import type { ActivityDto, AppInfoDto } from '../lib/types'

type Props = {
  activity: ActivityDto
  contextPct: number | null
  appInfo: AppInfoDto
}

function ctxColor(pct: number): string {
  if (pct < 50) return '#4ec94e'
  if (pct <= 75) return '#e5c07b'
  return '#f44444'
}

export default function StatusBar({ activity, contextPct, appInfo }: Props) {
  const label = activityLabel(activity)
  const projectText = appInfo.project_label ? 'project: ' + appInfo.project_label : 'no project'

  return (
    <div
      className="flex items-center gap-4 px-3 py-1 text-xs shrink-0"
      style={{ background: '#111', borderBottom: '1px solid #333', color: '#888' }}
    >
      <span style={{ color: '#569cd6' }}>{label}</span>
      <span>{projectText}</span>
      {contextPct !== null && (
        <span style={{ color: ctxColor(contextPct) }}>
          ctx: {contextPct}%
        </span>
      )}
      <span className="ml-auto" style={{ color: '#666' }}>{appInfo.app_name}</span>
    </div>
  )
}
