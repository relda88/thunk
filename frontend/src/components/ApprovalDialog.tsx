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
      className="rounded px-4 py-1 text-sm cursor-pointer font-mono bg-approve-bg text-accent-green border border-accent-green"
      onClick={onClick}
    >
      {label}
    </button>
  )
}

function RejectBtn({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      className="rounded px-4 py-1 text-sm cursor-pointer font-mono bg-reject-bg text-accent-red border border-accent-red"
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
      <span className="text-text-muted">affects: </span>
      <span className="text-text-muted">{impact.join(', ')}</span>
    </div>
  )
}

function EvidenceRow({ evidence }: { evidence: string[] }) {
  if (!evidence.length) return null
  return (
    <div className="text-sm mb-2">
      <span className="text-text-muted">evidence: </span>
      <span className="text-text-muted">{evidence.join(', ')}</span>
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
    <div className="fixed inset-0 flex items-center justify-center z-50 bg-black/70">
      <div className="rounded-md p-4 w-[90%] max-w-xl max-h-[80vh] overflow-y-auto bg-surface-overlay border border-border-strong text-text-primary">
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
                className={`mb-3 pb-3 ${i < dialog.actions.length - 1 ? 'border-b border-border' : ''}`}
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
              <span className="text-text-muted">goal: </span>{dialog.goal}
            </div>
            <div className="mb-3">
              {dialog.steps.map(([title, detail], i) => (
                <div key={i} className="mb-2">
                  <div className="text-sm">{i + 1}. {title}</div>
                  <div className="text-xs ml-4 text-text-muted">{detail}</div>
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
              <span className="text-text-muted">fact: </span>{dialog.fact}
            </div>
            <div className="text-sm mb-1">
              <span className="text-text-muted">category: </span>{dialog.category}
            </div>
            <div className="text-sm mb-1">
              <span className="text-text-muted">scope: </span>{dialog.scope ?? 'global'}
            </div>
            <div className="text-sm mb-1">
              <span className="text-text-muted">source: </span>{dialog.source}
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
