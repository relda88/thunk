export default function UserMessage({ text }: { text: string }) {
  return (
    <div
      className="max-w-[90%] self-end rounded px-3 py-2 whitespace-pre-wrap break-words text-sm"
      style={{ background: '#1e2a1e', borderLeft: '2px solid #4ec94e' }}
    >
      {text}
    </div>
  )
}
