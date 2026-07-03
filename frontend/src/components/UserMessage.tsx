export default function UserMessage({ text }: { text: string }) {
  return (
    <div className="max-w-[90%] self-end rounded px-3 py-2 whitespace-pre-wrap break-words text-sm bg-user-bg border-l-2 border-accent-green">
      {text}
    </div>
  )
}
