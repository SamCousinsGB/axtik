import test from 'node:test';
import assert from 'node:assert/strict';
import * as path from 'node:path';
import { containedBy, needsScopedWatcher, normalizeTargets, RequestLedger, snapshotSelectedTests, summaryLine, testPresentation, type ValidationReport, type TestObservation } from '../src/model';

test('a preview assertion pass cannot produce a qualified green Test Explorer result', () => {
  const result = {id:'DemoTest.test',path:'DemoTest.cls',line:1,status:'passed',message:'',durationMs:1} as TestObservation;
  assert.equal(testPresentation(result), 'skipped');
  assert.equal(testPresentation({...result,status:'failed'}), 'failed');
  assert.equal(testPresentation({...result,status:'blocked'}), 'errored');
  assert.equal(testPresentation({...result,status:'not_run'}), 'skipped');
});

test('project membership does not confuse sibling prefixes or traversal', () => {
  const root = path.resolve('project');
  assert.ok(containedBy(root, path.join(root, 'force-app', 'Demo.cls')));
  assert.ok(!containedBy(root, `${root}-other/Demo.cls`));
  assert.ok(!containedBy(root, path.join(root, '..', 'outside', 'Demo.cls')));
  assert.deepEqual(normalizeTargets([root, path.join(root, '.')]), [root]);
});

test('older requests cannot publish after a new run or source invalidation', () => {
  const ledger = new RequestLedger();
  const old = ledger.next('project');
  const current = ledger.next('project');
  assert.ok(!ledger.current('project', old));
  assert.ok(ledger.current('project', current));
  ledger.next('project');
  assert.ok(!ledger.current('project', current));
});

test('check summaries distinguish parsing from execution and certification', () => {
  const report = {command:'check',status:'checked',summary:{selectedFiles:3}} as ValidationReport;
  assert.match(summaryLine(report), /Tests were not run/);
  const failedCheck = {...report,status:'failed'} as ValidationReport;
  assert.match(summaryLine(failedCheck), /reported failed/);
  assert.match(summaryLine(failedCheck), /Tests were not run/);
  assert.doesNotMatch(summaryLine(failedCheck), /Experimental/);
  const preview = {...report,command:'validate',status:'preview_passed',summary:{selectedFiles:3,tests:1,passed:1,failed:0,blocked:0,errored:0}} as ValidationReport;
  assert.match(summaryLine(preview), /Experimental assurance/);
  assert.match(summaryLine(preview), /no Salesforce certification/);
});

test('Save and Continue can invalidate discovery without losing the requested test identity', () => {
  const identities = new Map([
    ['one', {id:'ServiceTest.first',path:'/project/ServiceTest.cls',root:'/project'}],
    ['two', {id:'ServiceTest.second',path:'/project/ServiceTest.cls',root:'/project'}],
  ]);
  const selected = snapshotSelectedTests([{id:'one'},{id:'two'}], identities, new Set(['two']));
  identities.clear(); // The save/change listener removes stale discovery entries.
  assert.equal(selected.length, 1);
  assert.equal(selected[0].identity.id, 'ServiceTest.first');
  assert.equal(selected[0].identity.path, '/project/ServiceTest.cls');
  assert.equal(selected[0].item.id, 'one');
});

test('external folders require their own watcher; sibling path prefixes are not workspace coverage', () => {
  const workspace = path.resolve('workspace');
  assert.equal(needsScopedWatcher(path.join(workspace, 'project'), [workspace]), false);
  assert.equal(needsScopedWatcher(workspace, []), true);
  assert.equal(needsScopedWatcher(`${workspace}-external`, [workspace]), true);
  assert.equal(needsScopedWatcher(path.dirname(workspace), [workspace]), true);
});
