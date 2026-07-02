export type ActivityDto =
  | { type: 'idle' }
  | { type: 'processing' }
  | { type: 'loading_model' }
  | { type: 'creating_context' }
  | { type: 'tokenizing' }
  | { type: 'prefilling' }
  | { type: 'generating'; mode: string | null }
  | { type: 'responding' }
  | { type: 'executing_tools'; tool: string; detail: string | null }
  | { type: 'awaiting_approval'; tool: string }

export type PendingActionDto = {
  tool_name: string
  summary: string
  risk: 'low' | 'medium' | 'high'
  reversible: boolean
  preview: string[]
  impact?: string[]
}

export type AnswerSourceDto = {
  type: string
}

export type AppInfoDto = {
  project_label: string | null
  app_name: string
}

export type RuntimeEventDto =
  | { type: 'assistant_message_started' }
  | { type: 'assistant_message_chunk'; chunk: string }
  | { type: 'assistant_message_finished' }
  | { type: 'system_message'; text: string }
  | { type: 'info_message'; text: string }
  | { type: 'failed'; message: string }
  | { type: 'tool_call_started'; name: string }
  | { type: 'tool_call_finished'; name: string; summary: string | null }
  | { type: 'file_read_finished'; path: string; line_count: number; content: string }
  | { type: 'direct_read_completed' }
  | { type: 'answer_ready'; source: AnswerSourceDto | null }
  | { type: 'activity_changed'; activity: ActivityDto }
  | { type: 'context_usage'; prompt_tokens: number; context_window_tokens: number }
  | { type: 'approval_required'; pending: PendingActionDto; evidence: string[]; impact: string[] }
  | { type: 'transaction_approval_required'; actions: PendingActionDto[]; impact: string[] }
  | { type: 'plan_approval_required'; goal: string; steps: [string, string][] }
  | { type: 'plan_approval_cleared' }
  | { type: 'memory_proposal_required'; fact: string; category: string; scope: string | null; source: string; delete: boolean }
  | { type: 'memory_proposal_cleared' }

export type DialogState =
  | { kind: 'mutation'; pending: PendingActionDto; evidence: string[]; impact: string[] }
  | { kind: 'transaction'; actions: PendingActionDto[]; impact: string[] }
  | { kind: 'plan'; goal: string; steps: [string, string][] }
  | { kind: 'memory'; fact: string; category: string; scope: string | null; source: string; delete: boolean }

export type ThreadItem =
  | { kind: 'user'; text: string; id: number }
  | { kind: 'assistant'; text: string; isStreaming: boolean; id: number }
  | { kind: 'system'; text: string; id: number }
  | { kind: 'error'; text: string; id: number }
  | { kind: 'file_read'; path: string; lineCount: number; content: string; id: number }
  | { kind: 'tool_activity'; name: string; status: 'running' | 'done' | 'failed'; summary: string | null; id: number }
