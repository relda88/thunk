import ReactMarkdown from 'react-markdown'

export default function AssistantMessage({ text, isStreaming }: { text: string; isStreaming: boolean }) {
  return (
    <div className="max-w-[90%] self-start rounded px-3 py-2 text-base bg-surface-raised border-l-2 border-accent-blue">
      <div className="prose prose-invert prose-sm max-w-none">
        <ReactMarkdown>{text}</ReactMarkdown>
      </div>
      {isStreaming && <span className="animate-pulse ml-px text-accent-blue">▊</span>}
    </div>
  )
}
