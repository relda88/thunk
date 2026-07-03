import type { HelpCommandDto } from '../lib/types'

export default function HelpMessage({ commands }: { commands: HelpCommandDto[] }) {
  return (
    <div
      className="self-start rounded px-3 py-2 text-xs max-w-[90%]"
      style={{ background: '#1e1e1e', borderLeft: '2px solid #555', color: '#888' }}
    >
      <table className="border-collapse">
        <tbody>
          {commands.map(c => (
            <tr key={c.name}>
              <td className="pr-4 py-0.5 align-top whitespace-nowrap" style={{ color: '#569cd6' }}>
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
