import { useEffect, useRef, useState } from 'react'
import { appInfo, onRuntimeEvent } from './lib/ipc'
import type { ActivityDto, AppInfoDto, DialogState, HelpCommandDto, RuntimeEventDto, ThreadItem } from './lib/types'
import { summarizeInfoMessage } from './lib/helpers'
import ChatView from './components/ChatView'
import StatusBar from './components/StatusBar'
import InputBar from './components/InputBar'
import ApprovalDialog from './components/ApprovalDialog'

export default function App() {
  const [thread, setThread] = useState<ThreadItem[]>([])
  const [dialog, setDialog] = useState<DialogState | null>(null)
  const [activity, setActivity] = useState<ActivityDto>({ type: 'idle' })
  const [contextPct, setContextPct] = useState<number | null>(null)
  const [info, setInfo] = useState<AppInfoDto>({ project_label: null, app_name: 'thunk' })
  const idRef = useRef(0)

  function nextId() {
    return idRef.current++
  }

  function addUserMessage(text: string) {
    const id = nextId()
    setThread(prev => [...prev, { kind: 'user', text, id }])
  }

  function addSystemMessage(text: string) {
    const id = nextId()
    setThread(prev => [...prev, { kind: 'system', text, id }])
  }

  function addHelpMessage(commands: HelpCommandDto[]) {
    const id = nextId()
    setThread(prev => [...prev, { kind: 'help', commands, id }])
  }

  useEffect(() => {
    appInfo().then(info => {
      setInfo(info)
      document.title = info.app_name + (info.project_label ? ' — ' + info.project_label : '')
    }).catch(() => {})

    const unlistenPromise = onRuntimeEvent((payload: RuntimeEventDto) => {
      switch (payload.type) {
        case 'assistant_message_started': {
          const id = idRef.current++
          setThread(prev => [...prev, { kind: 'assistant', text: '', isStreaming: true, id }])
          break
        }

        case 'assistant_message_chunk':
          setThread(prev => {
            const last = prev[prev.length - 1]
            if (last?.kind === 'assistant' && last.isStreaming) {
              return [...prev.slice(0, -1), { ...last, text: last.text + payload.chunk }]
            }
            return prev
          })
          break

        case 'assistant_message_finished':
          setThread(prev => {
            const last = prev[prev.length - 1]
            if (last?.kind === 'assistant' && last.isStreaming) {
              return [...prev.slice(0, -1), { ...last, isStreaming: false }]
            }
            return prev
          })
          break

        case 'system_message': {
          const id = idRef.current++
          setThread(prev => [...prev, { kind: 'system', text: payload.text, id }])
          break
        }

        case 'reset_ok': {
          const id = idRef.current++
          setThread([{ kind: 'system', text: 'Session cleared.', id }])
          break
        }

        case 'info_message': {
          const id = idRef.current++
          const summarized = summarizeInfoMessage(payload.text)
          setThread(prev => [...prev, { kind: 'system', text: summarized, id }])
          break
        }

        case 'failed': {
          setDialog(null)
          const id = idRef.current++
          setThread(prev => [...prev, { kind: 'error', text: 'Error: ' + payload.message, id }])
          setActivity({ type: 'idle' })
          break
        }

        case 'tool_call_started': {
          const id = idRef.current++
          setThread(prev => [...prev, {
            kind: 'tool_activity',
            name: payload.name,
            status: 'running',
            summary: null,
            id,
          }])
          break
        }

        case 'tool_call_finished':
          setThread(prev => {
            for (let i = prev.length - 1; i >= 0; i--) {
              const item = prev[i]
              if (item.kind === 'tool_activity' && item.name === payload.name && item.status === 'running') {
                const updated = [...prev]
                if (payload.name === 'read_file' && payload.summary !== null) {
                  updated[i] = { ...item, status: 'done', summary: null }
                } else if (payload.summary === null) {
                  updated[i] = { ...item, status: 'failed', summary: null }
                } else {
                  updated[i] = { ...item, status: 'done', summary: payload.summary }
                }
                return updated
              }
            }
            return prev
          })
          break

        case 'file_read_finished': {
          const id = idRef.current++
          setThread(prev => [...prev, {
            kind: 'file_read',
            path: payload.path,
            lineCount: payload.line_count,
            content: payload.content,
            id,
          }])
          break
        }

        case 'direct_read_completed':
          // no UI output needed; content arrives on file_read_finished
          break

        case 'answer_ready':
          setDialog(null)
          if (payload.source?.type === 'tool_limit_reached') {
            const id = idRef.current++
            setThread(prev => [...prev, {
              kind: 'system',
              text: 'Tool limit reached. Response may be incomplete.',
              id,
            }])
          }
          break

        case 'activity_changed':
          setActivity(payload.activity)
          break

        case 'context_usage': {
          const pct = Math.min(100, Math.floor(payload.prompt_tokens * 100 / payload.context_window_tokens))
          setContextPct(pct)
          break
        }

        case 'approval_required':
          setDialog({ kind: 'mutation', pending: payload.pending, evidence: payload.evidence, impact: payload.impact })
          break

        case 'transaction_approval_required':
          setDialog({ kind: 'transaction', actions: payload.actions, impact: payload.impact })
          break

        case 'plan_approval_required':
          setDialog({ kind: 'plan', goal: payload.goal, steps: payload.steps })
          break

        case 'plan_approval_cleared':
          setDialog(null)
          break

        case 'memory_proposal_required':
          setDialog({
            kind: 'memory',
            fact: payload.fact,
            category: payload.category,
            scope: payload.scope,
            source: payload.source,
            delete: payload.delete,
          })
          break

        case 'memory_proposal_cleared':
          setDialog(null)
          break
      }
    })

    return () => {
      unlistenPromise.then(fn => fn())
    }
  }, [])

  return (
    <div className="flex flex-col h-screen overflow-hidden font-mono" style={{ background: '#1a1a1a', color: '#d4d4d4' }}>
      <StatusBar activity={activity} contextPct={contextPct} appInfo={info} />
      <ChatView thread={thread} />
      <InputBar onUserMessage={addUserMessage} onSystemMessage={addSystemMessage} onHelp={addHelpMessage} />
      {dialog && <ApprovalDialog dialog={dialog} onClose={() => setDialog(null)} />}
    </div>
  )
}
