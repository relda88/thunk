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

export default function SystemMessage({ text }: { text: string }) {
  const { content, isToolResult } = extractToolResult(text)

  if (isToolResult) {
    return (
      <div
        className="self-start rounded px-3 py-1 text-xs max-w-[90%]"
        style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
      >
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
    <div
      className="self-start rounded px-3 py-1 text-xs whitespace-pre-wrap break-words max-w-[90%]"
      style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
    >
      {text}
    </div>
  )
}
