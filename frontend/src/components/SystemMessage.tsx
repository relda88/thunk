function extractToolResult(text: string): { content: string; isToolResult: boolean } {
  const envelopePattern = /^=== tool_result:.*$/m
  if (!envelopePattern.test(text)) {
    return { content: text, isToolResult: false }
  }
  const lines = text.split('\n')
  const filtered = lines.filter(
    l => !l.startsWith('=== tool_result:') && l !== '=== /tool_result ==='
  )
  return { content: filtered.join('\n').trim(), isToolResult: true }
}

function Kicker() {
  return <div className="text-[10px] text-text-faint uppercase tracking-wide mb-1">SYSTEM</div>
}

export default function SystemMessage({ text }: { text: string }) {
  const { content, isToolResult } = extractToolResult(text)

  if (isToolResult) {
    return (
      <div className="self-start rounded px-3 py-1 text-xs max-w-[90%] bg-surface border-l-2 border-border-accent text-text-muted">
        <Kicker />
        <pre
          className="whitespace-pre-wrap break-words overflow-auto"
          style={{ margin: 0, fontFamily: 'inherit', fontSize: 'inherit' }}
        >
          <code>{content}</code>
        </pre>
      </div>
    )
  }

  return (
    <div className="self-start rounded px-3 py-1 text-xs whitespace-pre-wrap break-words max-w-[90%] bg-surface border-l-2 border-border-accent text-text-muted">
      <Kicker />
      {text}
    </div>
  )
}
