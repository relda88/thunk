import type { HelpCommandDto } from '../lib/types'

export default function HelpMessage({ commands }: { commands: HelpCommandDto[] }) {
  return (
    <div className="self-start rounded px-3 py-2 text-xs max-w-[90%] bg-surface border-l-2 border-border-accent text-text-muted">
      <div className="text-[10px] text-text-faint uppercase tracking-wide mb-1">HELP</div>
      <table className="border-collapse">
        <tbody>
          {commands.map(c => (
            <tr key={c.name}>
              <td className="pr-4 py-0.5 align-top whitespace-nowrap text-accent-blue">
                {c.name}
              </td>
              <td className="py-0.5 align-top">{c.description}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}
