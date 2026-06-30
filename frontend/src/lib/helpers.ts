import type { ActivityDto } from './types'

export function activityLabel(a: ActivityDto): string {
  switch (a.type) {
    case 'idle': return 'Ready'
    case 'processing': return 'Processing'
    case 'loading_model': return 'loading_model'
    case 'creating_context': return 'creating_context'
    case 'tokenizing': return 'tokenizing'
    case 'prefilling': return 'prefilling'
    case 'generating': return a.mode ? a.mode + '...' : 'Generating...'
    case 'responding': return 'Responding'
    case 'executing_tools': return a.detail ? a.tool + ': ' + a.detail : a.tool + '...'
    case 'awaiting_approval': return 'Awaiting approval: ' + a.tool
  }
}

const TOOL_LABELS: Record<string, string> = {
  edit_file: 'Edit File',
  write_file: 'Create/Overwrite File',
  read_file: 'Read File',
  list_dir: 'List Directory',
  search_code: 'Search Code',
  shell: 'Shell Command',
  shell_read: 'Shell (Read)',
  git_status: 'Git Status',
  git_diff: 'Git Diff',
  git_log: 'Git Log',
  git_branch: 'Git Branch',
  lsp_definition: 'LSP Definition',
}

export function toolLabel(name: string): string {
  if (name.startsWith('mcp::')) return 'MCP: ' + name.slice(5)
  return TOOL_LABELS[name] ?? name
}
