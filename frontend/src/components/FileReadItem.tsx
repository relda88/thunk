export default function FileReadItem({ path, lineCount }: { path: string; lineCount: number }) {
  return (
    <div
      className="self-center rounded px-3 py-1 text-xs max-w-[90%] truncate"
      style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
    >
      read {path} ({lineCount} lines)
    </div>
  )
}
