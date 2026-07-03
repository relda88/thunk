import { activityLabel } from '../lib/helpers'
import type { ActivityDto, AppInfoDto } from '../lib/types'

type Props = {
  activity: ActivityDto
  contextPct: number | null
  appInfo: AppInfoDto
}

function ctxColor(pct: number): string {
  if (pct < 50) return 'text-accent-green'
  if (pct <= 75) return 'text-accent-yellow'
  return 'text-accent-red'
}

function Separator() {
  return <span className="text-border">·</span>
}

export default function StatusBar({ activity, contextPct, appInfo }: Props) {
  const label = activityLabel(activity)
  const projectText = appInfo.project_label ? 'project: ' + appInfo.project_label : 'no project'

  return (
    <div className="flex items-center gap-3 px-3 py-1 text-xs shrink-0 bg-surface-sunken border-b border-border text-text-muted">
      <span className="flex items-center gap-1.5 text-[10px] text-text-faint">
        {activity.type !== 'idle' && (
          <span className="inline-block w-1.5 h-1.5 rounded-full animate-pulse bg-accent-blue" />
        )}
        {label}
      </span>
      <Separator />
      <span className="text-text-primary">{projectText}</span>
      {contextPct !== null && (
        <>
          <Separator />
          <span className={ctxColor(contextPct)}>
            ctx: {contextPct}%
          </span>
        </>
      )}
      <span className="ml-auto text-text-faint">{appInfo.app_name}</span>
    </div>
  )
}
