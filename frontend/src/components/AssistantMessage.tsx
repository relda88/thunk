export default function AssistantMessage({ text, isStreaming }: { text: string; isStreaming: boolean }) {
  return (
    <div
      className="max-w-[90%] self-start rounded px-3 py-2 whitespace-pre-wrap break-words text-sm"
      style={{ background: '#2a2a2a', borderLeft: '2px solid #569cd6' }}
    >
      {text}
      {isStreaming && <span className="animate-pulse ml-px" style={{ color: '#569cd6' }}>▊</span>}
    </div>
  )
}
