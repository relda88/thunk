import { useState } from 'react'

export default function FileReadItem({ path, lineCount, content }: { path: string; lineCount: number; content: string }) {
  const [expanded, setExpanded] = useState(false)

  return (
    <div className="self-start rounded px-3 py-1 text-xs max-w-[90%] bg-surface border-l-2 border-border-accent text-text-muted">
      <div className="text-[10px] text-text-faint uppercase tracking-wide mb-1">FILE</div>
      <div
        className="cursor-pointer flex items-center gap-1"
        onClick={() => setExpanded(e => !e)}
      >
        <span>{expanded ? '▲' : '▼'}</span>
        <span>read {path} ({lineCount} lines)</span>
      </div>
      {expanded && (
        <pre className="whitespace-pre-wrap text-xs overflow-x-auto">{content}</pre>
      )}
    </div>
  )
}
