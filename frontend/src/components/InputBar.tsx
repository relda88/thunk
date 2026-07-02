import { useEffect, useRef, useState } from 'react'
import { submitMessage, runCommand } from '../lib/ipc'

type Props = {
  onUserMessage: (text: string) => void
  onSystemMessage: (text: string) => void
}

export default function InputBar({ onUserMessage, onSystemMessage }: Props) {
  const [value, setValue] = useState('')
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    if (value === '' && textareaRef.current) {
      textareaRef.current.style.height = 'auto'
    }
  }, [value])

  function handleChange(e: React.ChangeEvent<HTMLTextAreaElement>) {
    setValue(e.target.value)
    const el = e.target
    el.style.height = 'auto'
    el.style.height = Math.min(el.scrollHeight, 144) + 'px'
  }

  async function handleKeyDown(e: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault()
      const text = value.trim()
      setValue('')
      if (!text) return
      if (text.startsWith('/')) {
        try {
          await runCommand(text)
        } catch (err) {
          onSystemMessage(String(err))
        }
      } else {
        onUserMessage(text)
        try {
          await submitMessage(text)
        } catch (err) {
          onSystemMessage('Failed to send: ' + String(err))
        }
      }
    }
  }

  return (
    <div
      className="shrink-0 p-2"
      style={{ borderTop: '1px solid #333' }}
    >
      <textarea
        ref={textareaRef}
        value={value}
        onChange={handleChange}
        onKeyDown={handleKeyDown}
        rows={2}
        placeholder="Ask thunk anything... (Enter to send, Shift+Enter for newline)"
        className="w-full rounded px-3 py-2 text-sm outline-none resize-none font-mono"
        style={{
          background: '#2a2a2a',
          color: '#d4d4d4',
          border: '1px solid #444',
          maxHeight: '144px',
        }}
        onFocus={e => { e.target.style.borderColor = '#569cd6' }}
        onBlur={e => { e.target.style.borderColor = '#444' }}
      />
    </div>
  )
}
