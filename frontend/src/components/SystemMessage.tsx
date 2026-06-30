export default function SystemMessage({ text }: { text: string }) {
  return (
    <div
      className="self-center rounded px-3 py-1 text-xs whitespace-pre-wrap break-words max-w-[90%]"
      style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
    >
      {text}
    </div>
  )
}
