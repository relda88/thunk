import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { AppInfoDto, HelpCommandDto, RuntimeEventDto } from './types'

export function submitMessage(text: string): Promise<void> {
  return invoke('submit', { text })
}

export function runCommand(input: string): Promise<void> {
  return invoke('run_command', { input })
}

export function approve(): Promise<void> {
  return invoke('approve')
}

export function reject(): Promise<void> {
  return invoke('reject')
}

export function planApprove(): Promise<void> {
  return invoke('plan_approve')
}

export function planAbandon(): Promise<void> {
  return invoke('plan_abandon')
}

export function memoryApprove(): Promise<void> {
  return invoke('memory_approve')
}

export function memoryReject(): Promise<void> {
  return invoke('memory_reject')
}

export function appInfo(): Promise<AppInfoDto> {
  return invoke('app_info')
}

export function getHelpCommands(): Promise<HelpCommandDto[]> {
  return invoke('get_help_commands')
}

export function onRuntimeEvent(
  handler: (event: RuntimeEventDto) => void
): Promise<() => void> {
  return listen<RuntimeEventDto>('runtime-event', (event) => {
    handler(event.payload)
  })
}
