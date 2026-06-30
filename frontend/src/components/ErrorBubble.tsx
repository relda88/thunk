export default function ErrorBubble({ text }: { text: string }) {
  return (
    <div
      className="max-w-[90%] self-start rounded px-3 py-2 whitespace-pre-wrap break-words text-sm"
      style={{ background: '#2a1a1a', borderLeft: '2px solid #f44444', color: '#f99999' }}
    >
      {text}
    </div>
  )
}
