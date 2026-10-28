#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {execFileSync} from 'node:child_process';
import {assertConserved,compareBaseline,negativeMutation,portable,scanTests,sha256,summarizeBlockers} from './corpus-lib.mjs';

const project = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const options = {};
for (let i=2;i<process.argv.length;i++) {
  const arg = process.argv[i];
  if (arg === '--require-all') options.requireAll = true;
  else if (['--repo','--engine','--output','--baseline','--timeout-ms','--environment'].includes(arg) && process.argv[i+1]) options[arg.slice(2)] = process.argv[++i];
  else throw new Error(`Unknown or incomplete argument ${arg}. Usage: node scripts/corpus.mjs --repo <pinned clone> --engine <apx> [--output <history directory>] [--baseline <previous JSON>] [--require-all]`);
}
if (!options.repo || !options.engine) throw new Error('--repo and --engine are required. The runner never downloads or builds either implicitly.');
const repo = await fs.realpath(path.resolve(options.repo));
const engine = await fs.realpath(path.resolve(options.engine));
const environmentPath = options.environment ? await fs.realpath(path.resolve(options.environment)) : undefined;
const environmentSha256 = environmentPath ? sha256(await fs.readFile(environmentPath)) : null;
const output = path.resolve(options.output ?? path.join(project,'.corpus-history'));
const pin = JSON.parse(await fs.readFile(path.join(project,'corpus','apex-recipes.pin.json'),'utf8'));
const timeoutMs = Number(options['timeout-ms'] ?? 120000);
if (!Number.isFinite(timeoutMs) || timeoutMs < 100 || timeoutMs > 3600000) throw new Error('Invalid --timeout-ms (100..3600000).');
const git = (...args) => execFileSync('git',['-C',repo,...args],{encoding:'utf8',timeout:30000,maxBuffer:16*1024*1024,windowsHide:true}).trim();
const normalizeRemote = s => s.replace(/^git@github\.com:/,'https://github.com/').replace(/\.git$/,'').replace(/\/$/,'').toLowerCase();

async function verifySource() {
  if (git('rev-parse','HEAD') !== pin.revision) throw new Error(`Expected upstream revision ${pin.revision}. Refusing a different corpus.`);
  if (git('rev-parse','HEAD^{tree}') !== pin.gitTree) throw new Error('Upstream tree identity changed.');
  if (normalizeRemote(git('remote','get-url','origin')) !== normalizeRemote(pin.repository)) throw new Error('Upstream origin is not the pinned official repository.');
  const dirty = git('status','--porcelain','--untracked-files=all');
  if (dirty) throw new Error(`Upstream source must be unchanged, including untracked files:\n${dirty}`);
  const files = git('ls-files','-z').split('\0').filter(Boolean).filter(f => /\.(cls|trigger)$/i.test(f)).sort();
  if (files.length !== pin.sourceFiles.length || files.some((f,i) => f !== pin.sourceFiles[i].path)) throw new Error('Apex source-file identity changed.');
  const tests = [];
  for (const file of pin.sourceFiles) {
    const source = await fs.readFile(path.join(repo,file.path),'utf8');
    if (sha256(source.replaceAll('\r\n','\n')) !== file.sha256) throw new Error(`Pinned source hash mismatch: ${file.path}`);
    tests.push(...scanTests(source,file.path));
  }
  if (pin.tests.length !== 322 || tests.length !== 322) throw new Error('This corpus must account for exactly 322 tests.');
  assertConserved(pin.tests,tests,'Independent source inventory');
  return tests;
}

await fs.mkdir(output,{recursive:true});
const sourceTests = await verifySource();
const {runEngine} = await import('../packages/runner/lib/index.js').catch(() => {throw new Error('Build the shared runner first: npm run build --workspace @apx/runner');});
const engineSha256 = sha256(await fs.readFile(engine));
const run = (command,root=repo,filter) => runEngine({enginePath:engine,command,paths:[root],projectRoot:root,filter,environmentPath,timeoutMs,maxOutputBytes:32*1024*1024});
console.log(`Auditing ${pin.repository}@${pin.revision.slice(0,12)}: ${pin.tests.length} pinned tests, ${pin.sourceFiles.length} Apex files.`);
const check = await run('check');
assertConserved(pin.tests,check.tests,'Native discovery');
if (check.status !== 'checked' || check.exitCode !== 0 || check.tests.some(t => t.status !== 'not_run')) throw new Error('Native discovery must check all tests without executing them.');
const report = await run('validate');
assertConserved(pin.tests,report.tests,'Native execution');
for (const native of [check,report]) {
  const files = native.files.map(f => portable(path.relative(repo,f.path))).sort();
  if (files.length !== pin.sourceFiles.length || files.some((f,i) => f !== pin.sourceFiles[i].path) || native.files.some(f => !f.selected)) throw new Error('Native run narrowed the pinned source set.');
}
const tests = report.tests.map(t => ({...t,path:portable(path.relative(repo,t.path)),message:t.message.replaceAll(repo,'${APEX_RECIPES}')})).sort((a,b) => a.id.localeCompare(b.id));
const expected = new Map(pin.tests.map(t => [t.id,t]));
for (const test of tests) if (test.path !== expected.get(test.id).path) throw new Error(`Native test moved to a different source file: ${test.id}`);
let baseline;
if (options.baseline) baseline = JSON.parse(await fs.readFile(path.resolve(options.baseline),'utf8'));
else {
  try { const pointer=JSON.parse(await fs.readFile(path.join(output,'latest.json'),'utf8')); baseline=JSON.parse(await fs.readFile(path.join(output,path.basename(pointer.report)),'utf8')); }
  catch (error) { if (error.code !== 'ENOENT') throw error; }
}
if (baseline && (baseline.revision !== pin.revision || baseline.repository !== pin.repository)) throw new Error('Baseline belongs to a different repository or revision.');
const diff = baseline ? compareBaseline(baseline,{tests,engineSha256}) : {previousEngineSha256:null,transitions:[],gainedPasses:tests.filter(t=>t.status==='passed').map(t=>t.id),regressions:[],blockerChanges:[]};
const passing = tests.filter(t => t.status === 'passed');
const mutations = [];
let copy;
try {
  if (passing.length) {
    copy = await fs.mkdtemp(path.join(output,'mutation-'));
    for (const entry of ['force-app','config','sfdx-project.json','LICENSE.md']) await fs.cp(path.join(repo,entry),path.join(copy,entry),{recursive:true});
  }
  for (const [index,test] of passing.entries()) {
    const source = await fs.readFile(path.join(repo,test.path),'utf8');
    const method = sourceTests.find(t => t.id === test.id);
    const mutation = negativeMutation(source,method);
    const target = path.join(copy,test.path);
    await fs.writeFile(target,mutation.source);
    let result;
    try {
      const observed = await run('test',copy,test.id);
      assertConserved([test],observed.tests,`Mutant ${test.id}`);
      const failed = observed.tests[0];
      result = {id:test.id,family:method.family,kind:mutation.kind,killed:observed.exitCode === 1 && failed.status === 'failed' && /assertion/i.test(failed.message),status:failed.status,message:failed.message.replaceAll(copy,'${MUTANT}')};
    } catch (error) { result={id:test.id,family:method.family,kind:mutation.kind,killed:false,status:'harness-error',message:String(error.message)}; }
    finally { await fs.writeFile(target,source); }
    mutations.push(result);
    console.log(`Negative mutation ${index+1}/${passing.length}: ${test.id} ${result.killed?'killed':'NOT KILLED'} (${mutation.kind}).`);
  }
} finally {
  if (copy) {
    const realCopy=await fs.realpath(copy), realOutput=await fs.realpath(output);
    if (path.dirname(realCopy) !== realOutput || !path.basename(realCopy).startsWith('mutation-')) throw new Error('Mutation cleanup escaped its generated directory.');
    await fs.rm(realCopy,{recursive:true,force:true});
  }
}
await verifySource();
if (sha256(await fs.readFile(engine)) !== engineSha256) throw new Error('Engine binary changed during the corpus run. Re-run with a stable binary.');
if (environmentPath && sha256(await fs.readFile(environmentPath)) !== environmentSha256) throw new Error('Environment changed during the corpus run. Re-run with a stable fixture.');
const mutationFailures = mutations.filter(m=>!m.killed);
const record = {
  schemaVersion:1,createdAt:new Date().toISOString(),repository:pin.repository,revision:pin.revision,gitTree:pin.gitTree,
  engineSha256,engineVersion:report.engineVersion,sourceUnchanged:true,
  environment:environmentPath ? {name:path.basename(environmentPath),sha256:environmentSha256,qualification:'Explicit synthetic local profile; not a Salesforce org export.'} : null,
  qualification:'Experimental local execution. No Salesforce oracle comparison; no deployment certification.',
  check:{status:check.status,exitCode:check.exitCode,summary:check.summary},
  validation:{status:report.status,exitCode:report.exitCode,assurance:report.assurance,summary:report.summary},
  conservation:{expectedTests:322,discoveredTests:check.tests.length,executedTests:tests.length,expectedFiles:pin.sourceFiles.length,verified:true},
  tests,blockerGroups:summarizeBlockers(tests),diff,mutations,
  gate:{regressionFree:diff.regressions.length===0,negativeMutationsKilled:mutationFailures.length===0,allTestsPassed:passing.length===322,sentinelOnlyTests:mutations.filter(m=>m.kind==='entry-sentinel').map(m=>m.id)},
};
const filename=`${record.createdAt.replaceAll(':','-')}-${engineSha256.slice(0,12)}.json`;
await fs.writeFile(path.join(output,filename),JSON.stringify(record,null,2)+'\n',{flag:'wx'});
await fs.writeFile(path.join(output,'latest.json'),JSON.stringify({report:filename},null,2)+'\n');
console.log(JSON.stringify({report:path.join(output,filename),summary:report.summary,regressions:diff.regressions.length,mutationFailures:mutationFailures.length,qualification:record.qualification},null,2));
if (diff.regressions.length || mutationFailures.length || (options.requireAll && passing.length!==322)) process.exitCode=1;
