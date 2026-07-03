export default function ErrorBubble({ text }: { text: string }) {
  return (
    <div className="max-w-[90%] self-start rounded px-3 py-2 whitespace-pre-wrap break-words text-sm bg-error-bg border-l-2 border-accent-red text-red-soft">
      {text}
    </div>
  )
}
