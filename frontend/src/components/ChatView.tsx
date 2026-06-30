import { useLayoutEffect, useRef } from 'react'
import type { ThreadItem } from '../lib/types'
import UserMessage from './UserMessage'
import AssistantMessage from './AssistantMessage'
import SystemMessage from './SystemMessage'
import ErrorBubble from './ErrorBubble'
import FileReadItem from './FileReadItem'
import ToolActivityItem from './ToolActivityItem'

function renderItem(item: ThreadItem) {
  switch (item.kind) {
    case 'user':
      return <UserMessage key={item.id} text={item.text} />
    case 'assistant':
      return <AssistantMessage key={item.id} text={item.text} isStreaming={item.isStreaming} />
    case 'system':
      return <SystemMessage key={item.id} text={item.text} />
    case 'error':
      return <ErrorBubble key={item.id} text={item.text} />
    case 'file_read':
      return <FileReadItem key={item.id} path={item.path} lineCount={item.lineCount} />
    case 'tool_activity':
      return <ToolActivityItem key={item.id} name={item.name} status={item.status} summary={item.summary} />
  }
}

export default function ChatView({ thread }: { thread: ThreadItem[] }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const atBottomRef = useRef(true)

  function handleScroll() {
    const el = containerRef.current
    if (!el) return
    atBottomRef.current = el.scrollTop + el.clientHeight >= el.scrollHeight - 10
  }

  useLayoutEffect(() => {
    const el = containerRef.current
    if (!el || !atBottomRef.current) return
    el.scrollTop = el.scrollHeight
  }, [thread])

  return (
    <div
      ref={containerRef}
      onScroll={handleScroll}
      className="flex-1 overflow-y-auto p-3 flex flex-col gap-2"
    >
      {thread.map(item => renderItem(item))}
    </div>
  )
}
