import { useEffect, useRef, useState } from 'react'
import { submitMessage, runCommand, getHelpCommands } from '../lib/ipc'
import type { HelpCommandDto } from '../lib/types'

type Props = {
  onUserMessage: (text: string) => void
  onSystemMessage: (text: string) => void
  onHelp: (commands: HelpCommandDto[]) => void
}

export default function InputBar({ onUserMessage, onSystemMessage, onHelp }: Props) {
  const [value, setValue] = useState('')
  const [commandInFlight, setCommandInFlight] = useState(false)
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
      if (commandInFlight) return
      const text = value.trim()
      setValue('')
      if (!text) return
      if (text.startsWith('/')) {
        const commandName = text.split(/\s+/)[0]
        setCommandInFlight(true)
        try {
          if (commandName === '/help') {
            const commands = await getHelpCommands()
            onHelp(commands)
          } else {
            await runCommand(text)
          }
        } catch (err) {
          onSystemMessage(String(err))
        } finally {
          setCommandInFlight(false)
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
        disabled={commandInFlight}
        rows={2}
        placeholder="Ask thunk anything... (Enter to send, Shift+Enter for newline)"
        className="w-full rounded px-3 py-2 text-sm outline-none resize-none font-mono disabled:opacity-50"
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
