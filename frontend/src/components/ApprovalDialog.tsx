import { approve, reject, planApprove, planAbandon, memoryApprove, memoryReject } from '../lib/ipc'
import type { DialogState } from '../lib/types'
import PendingActionCard from './PendingActionCard'

type Props = {
  dialog: DialogState
  onClose: () => void
}

function ApproveBtn({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      className="rounded px-4 py-1 text-sm cursor-pointer font-mono"
      style={{ background: '#2a4a2a', color: '#4ec94e', border: '1px solid #4ec94e' }}
      onClick={onClick}
    >
      {label}
    </button>
  )
}

function RejectBtn({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      className="rounded px-4 py-1 text-sm cursor-pointer font-mono"
      style={{ background: '#4a2a2a', color: '#f44444', border: '1px solid #f44444' }}
      onClick={onClick}
    >
      {label}
    </button>
  )
}

function ImpactRow({ impact }: { impact: string[] }) {
  if (!impact.length) return null
  return (
    <div className="text-sm mb-2">
      <span style={{ color: '#888' }}>affects: </span>
      <span style={{ color: '#888' }}>{impact.join(', ')}</span>
    </div>
  )
}

function EvidenceRow({ evidence }: { evidence: string[] }) {
  if (!evidence.length) return null
  return (
    <div className="text-sm mb-2">
      <span style={{ color: '#888' }}>evidence: </span>
      <span style={{ color: '#888' }}>{evidence.join(', ')}</span>
    </div>
  )
}

export default function ApprovalDialog({ dialog, onClose }: Props) {
  async function doApprove() {
    onClose()
    try { await approve() } catch (_) { /* event handler surfaces errors */ }
  }
  async function doReject() {
    onClose()
    try { await reject() } catch (_) { }
  }
  async function doPlanApprove() {
    onClose()
    try { await planApprove() } catch (_) { }
  }
  async function doPlanAbandon() {
    onClose()
    try { await planAbandon() } catch (_) { }
  }
  async function doMemoryApprove() {
    onClose()
    try { await memoryApprove() } catch (_) { }
  }
  async function doMemoryReject() {
    onClose()
    try { await memoryReject() } catch (_) { }
  }

  return (
    <div
      className="fixed inset-0 flex items-center justify-center z-50"
      style={{ background: 'rgba(0,0,0,0.7)' }}
    >
      <div
        className="rounded-md p-4 w-[90%] max-w-xl max-h-[80vh] overflow-y-auto"
        style={{ background: '#252525', border: '1px solid #444', color: '#d4d4d4' }}
      >
        {dialog.kind === 'mutation' && (
          <>
            <h3 className="text-sm font-bold mb-3">Approve action?</h3>
            <PendingActionCard pending={dialog.pending} />
            <EvidenceRow evidence={dialog.evidence} />
            <ImpactRow impact={dialog.impact} />
            <div className="flex gap-2 mt-3">
              <ApproveBtn label="Approve" onClick={doApprove} />
              <RejectBtn label="Reject" onClick={doReject} />
            </div>
          </>
        )}

        {dialog.kind === 'transaction' && (
          <>
            <h3 className="text-sm font-bold mb-3">
              Approve transaction ({dialog.actions.length} actions)?
            </h3>
            {dialog.actions.map((action, i) => (
              <div
                key={i}
                className="mb-3 pb-3"
                style={{ borderBottom: i < dialog.actions.length - 1 ? '1px solid #333' : undefined }}
              >
                <PendingActionCard pending={action} index={i} />
              </div>
            ))}
            <ImpactRow impact={dialog.impact} />
            <div className="flex gap-2 mt-3">
              <ApproveBtn label="Approve all" onClick={doApprove} />
              <RejectBtn label="Reject" onClick={doReject} />
            </div>
          </>
        )}

        {dialog.kind === 'plan' && (
          <>
            <h3 className="text-sm font-bold mb-3">Approve plan?</h3>
            <div className="text-sm mb-3">
              <span style={{ color: '#888' }}>goal: </span>{dialog.goal}
            </div>
            <div className="mb-3">
              {dialog.steps.map(([title, detail], i) => (
                <div key={i} className="mb-2">
                  <div className="text-sm">{i + 1}. {title}</div>
                  <div className="text-xs ml-4" style={{ color: '#888' }}>{detail}</div>
                </div>
              ))}
            </div>
            <div className="flex gap-2 mt-3">
              <ApproveBtn label="Approve" onClick={doPlanApprove} />
              <RejectBtn label="Abandon" onClick={doPlanAbandon} />
            </div>
          </>
        )}

        {dialog.kind === 'memory' && (
          <>
            <h3 className="text-sm font-bold mb-3">
              {dialog.delete ? 'Forget this fact:' : 'Propose memory:'}
            </h3>
            <div className="text-sm mb-1">
              <span style={{ color: '#888' }}>fact: </span>{dialog.fact}
            </div>
            <div className="text-sm mb-1">
              <span style={{ color: '#888' }}>category: </span>{dialog.category}
            </div>
            <div className="text-sm mb-1">
              <span style={{ color: '#888' }}>scope: </span>{dialog.scope ?? 'global'}
            </div>
            <div className="text-sm mb-1">
              <span style={{ color: '#888' }}>source: </span>{dialog.source}
            </div>
            <div className="flex gap-2 mt-3">
              <ApproveBtn label="Approve" onClick={doMemoryApprove} />
              <RejectBtn label="Reject" onClick={doMemoryReject} />
            </div>
          </>
        )}
      </div>
    </div>
  )
}
