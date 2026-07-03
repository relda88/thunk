import type { ActivityDto } from './types'

export function activityLabel(a: ActivityDto): string {
  switch (a.type) {
    case 'idle': return 'Ready'
    case 'processing': return 'Processing'
    case 'loading_model': return 'Loading model'
    case 'creating_context': return 'Creating context'
    case 'tokenizing': return 'Tokenizing'
    case 'prefilling': return 'Prefilling'
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

export function summarizeInfoMessage(text: string): string {
  const headerPattern = /^=== tool_result:\s*(\S+)\s*===$/m
  const headerMatch = headerPattern.exec(text)
  if (!headerMatch) return text

  const name = headerMatch[1]
  const bodyLines = text
    .slice(headerMatch.index + headerMatch[0].length)
    .split('\n')
    .map(l => l.trim())
    .filter(l => l.length > 0 && l !== '=== /tool_result ===')

  const firstLine = bodyLines[0] ?? ''
  const lineCountMatch = /(\d+)\s+lines?/i.exec(firstLine)
  if (lineCountMatch) {
    return 'tool: ' + name + ' → ' + lineCountMatch[1] + ' lines'
  }

  const detail = firstLine.length > 60 ? firstLine.slice(0, 60) + '...' : firstLine
  return detail ? 'tool: ' + name + ' → ' + detail : 'tool: ' + name
}
