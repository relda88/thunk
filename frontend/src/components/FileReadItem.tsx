import { useState } from 'react'

export default function FileReadItem({ path, lineCount, content }: { path: string; lineCount: number; content: string }) {
  const [expanded, setExpanded] = useState(false)

  return (
    <div
      className="self-start rounded px-3 py-1 text-xs max-w-[90%]"
      style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
    >
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
