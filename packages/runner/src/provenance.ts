/** Versioned observations of local models. These are not platform conformance evidence. */
export const capabilityContractIds = ['apx.native-method.v1', 'apx.native-constructor.v1', 'apx.soql.v1', 'apx.sosl.v1', 'apx.dml.v1'] as const;
export type CapabilityContractId = typeof capabilityContractIds[number];
export interface CapabilityOperation {
  contractId: CapabilityContractId;
  operation: string;
  phase: 'setup' | 'test';
  returned: number;
  failed: number;
  blocked: number;
}
export interface CapabilityAssumption {
  id: string;
  kind: 'synthetic' | 'host-derived';
  scope: 'configured-for-test' | 'reached-operation';
}
export interface TestProvenance {
  schemaVersion: 1;
  conformance: 'unqualified';
  inventoryComplete: false;
  truncated: boolean;
  operations: CapabilityOperation[];
  assumptions: CapabilityAssumption[];
}
export interface ReportProvenance {
  schemaVersion: 1;
  catalogueVersion: 'apx-capabilities-v1';
  conformance: 'unqualified';
  compiler: {coverage: 'bounded-semantic-subset'; conformance: 'unqualified'};
  inventory: {scope: 'reached-native-dispatch'; complete: false; limitations: string[]};
  contracts: Array<{id: CapabilityContractId; implementation: 'partial-local-model'; basis: 'local-implementation'; conformance: 'unqualified'; independentEvidence: never[]}>;
}

const object = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const exact = (v: unknown, keys: string[]): v is Record<string, unknown> => object(v) && Object.keys(v).length === keys.length && keys.every(k => Object.hasOwn(v, k));
const count = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
const text = (v: unknown): v is string => typeof v === 'string' && v.length > 0 && Buffer.byteLength(v, 'utf8') <= 256 && !/[\u0000-\u001f\u007f]/.test(v);
const assumptions: Record<string, [string, string]> = {
  'explicit-environment-fixture': ['synthetic', 'configured-for-test'],
  'fixture-clock': ['synthetic', 'reached-operation'],
  'host-clock': ['host-derived', 'reached-operation'],
  'local-record-store': ['synthetic', 'reached-operation'],
  'local-test-lifecycle': ['synthetic', 'reached-operation'],
  'local-limit-model': ['synthetic', 'reached-operation'],
  'local-platform-context': ['synthetic', 'reached-operation'],
};

/** Old protocol-2 reports may omit provenance; partial or self-promoted metadata is rejected. */
export function validProvenance(report: Record<string, unknown>): boolean {
  if (!Array.isArray(report.tests)) return false;
  const p = report.provenance;
  if (p === undefined) return report.tests.every(t => object(t) && t.provenance === undefined);
  if (!exact(p, ['schemaVersion', 'catalogueVersion', 'conformance', 'compiler', 'inventory', 'contracts']) ||
      p.schemaVersion !== 1 || p.catalogueVersion !== 'apx-capabilities-v1' || p.conformance !== 'unqualified' ||
      !exact(p.compiler, ['coverage', 'conformance']) || p.compiler.coverage !== 'bounded-semantic-subset' || p.compiler.conformance !== 'unqualified' ||
      !exact(p.inventory, ['scope', 'complete', 'limitations']) || p.inventory.scope !== 'reached-native-dispatch' || p.inventory.complete !== false ||
      !Array.isArray(p.inventory.limitations) || p.inventory.limitations.length === 0 || p.inventory.limitations.length > 16 || !p.inventory.limitations.every(text) ||
      !Array.isArray(p.contracts) || p.contracts.length !== capabilityContractIds.length) return false;
  const ids = new Set<string>();
  for (const c of p.contracts) {
    if (!exact(c, ['id', 'implementation', 'basis', 'conformance', 'independentEvidence']) ||
        !capabilityContractIds.includes(c.id as CapabilityContractId) || ids.has(c.id as string) ||
        c.implementation !== 'partial-local-model' || c.basis !== 'local-implementation' || c.conformance !== 'unqualified' ||
        !Array.isArray(c.independentEvidence) || c.independentEvidence.length !== 0) return false;
    ids.add(c.id as string);
  }
  for (const test of report.tests) {
    if (!object(test)) return false;
    const t = test.provenance;
    if (!exact(t, ['schemaVersion', 'conformance', 'inventoryComplete', 'truncated', 'operations', 'assumptions']) ||
        t.schemaVersion !== 1 || t.conformance !== 'unqualified' || t.inventoryComplete !== false || typeof t.truncated !== 'boolean' ||
        !Array.isArray(t.operations) || t.operations.length > 256 || !Array.isArray(t.assumptions) || t.assumptions.length > Object.keys(assumptions).length) return false;
    if (test.status === 'not_run' && (t.operations.length || t.assumptions.length || t.truncated)) return false;
    const operations = new Set<string>();
    for (const op of t.operations) {
      if (!exact(op, ['contractId', 'operation', 'phase', 'returned', 'failed', 'blocked']) || !ids.has(op.contractId as string) ||
          !text(op.operation) || !['setup', 'test'].includes(op.phase as string) || !count(op.returned) || !count(op.failed) || !count(op.blocked)) return false;
      const total = op.returned + op.failed + op.blocked;
      const key = JSON.stringify([op.contractId, op.operation, op.phase]);
      if (!Number.isSafeInteger(total) || total === 0 || operations.has(key)) return false;
      operations.add(key);
    }
    const seen = new Set<string>();
    for (const a of t.assumptions) {
      if (!exact(a, ['id', 'kind', 'scope']) || typeof a.id !== 'string' || !Object.hasOwn(assumptions, a.id) || seen.has(a.id)) return false;
      const [kind, scope] = assumptions[a.id]!;
      if (a.kind !== kind || a.scope !== scope) return false;
      seen.add(a.id);
    }
  }
  return true;
}

export function describeTestProvenance(test: {provenance?: TestProvenance}): string {
  const p = test.provenance;
  if (!p) return 'Model evidence unavailable from this engine; Salesforce conformance unqualified.';
  const assumptions = p.assumptions.length ? p.assumptions.map(a => a.id).join(', ') : 'none recorded';
  return `Local model: ${p.operations.length} observed operation entries; assumptions: ${assumptions}. Partial${p.truncated ? ', truncated' : ''} inventory; Salesforce conformance unqualified.`;
}
