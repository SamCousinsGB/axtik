import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {parseOptions,validateManifest,deployArguments,verifyOrgIdentity,nativeObservation,salesforceObservation,satisfiesExpected,compareObservations,runProcess,redact,writeReceipt,snapshot,verifyCapturedInputs} from './salesforce-compare.mjs';

const root=path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const scenario={id:'positive',project:'positive',local:{command:'test',paths:['force-app'],filter:'Probe.sample'},salesforce:{testClasses:['Probe']},expected:{compile:'pass',tests:'pass'}};
const negative={...scenario,expected:{compile:'pass',tests:'fail',failure:{category:'governor',messageIncludes:['System.LimitException','DML rows']}}};
const compiler={...scenario,local:{command:'check',paths:['force-app']},salesforce:{testClasses:[]},expected:{compile:'fail',tests:'not_run',failure:{category:'compile',location:{path:'force-app/Bad.cls',line:4},messageIncludes:['String','Integer']}}};
const orgId='00D000000000001AAA';
const org=(patch={})=>({status:0,result:{done:true,totalSize:1,records:[{Id:orgId,IsSandbox:true,TrialExpirationDate:null,...patch}]}});
const oracle=(failed=false)=>({status:failed?1:0,result:{id:'0Af000000000001AAA',done:true,checkOnly:true,status:failed?'Failed':'Succeeded',success:!failed,numberComponentErrors:0,details:{componentFailures:[],runTestResult:{numTestsRun:1,numFailures:failed?1:0,successes:failed?[]:[{name:'Probe',methodName:'sample'}],failures:failed?[{name:'Probe',methodName:'sample',message:'System.LimitException: Too many DML rows: 10001'}]:[]}}}});
const checked={status:'checked',exitCode:0,diagnostics:[]};
const execution=(status='passed')=>({status:status==='passed'?'preview_passed':'failed',exitCode:status==='passed'?4:1,inputFingerprint:'a'.repeat(64),diagnostics:status==='failed'?[{severity:'error',code:'APX_GOVERNOR',message:'System.LimitException: Too many DML rows: 10001'}]:[],tests:[{id:'Probe.sample',status,message:status==='passed'?'preview':'System.LimitException: Too many DML rows: 10001',assertions:{executed:1,passed:status==='passed'?1:0,failed:status==='passed'?0:1},setupAssertions:{executed:0,passed:0,failed:0}}]});
async function temporary(t) {const dir=await fs.mkdtemp(path.join(os.tmpdir(),'axtik-compare-'));t.after(async()=>{assert.equal(path.dirname(dir),os.tmpdir());await fs.rm(dir,{recursive:true,force:true});});return dir;}

test('live mode requires explicit target identity, authorization and one CLI entry',()=>{
  assert.equal(parseOptions([]).mode,'plan');
  for(const args of [['run'],['run','--engine','engine'],['run','--engine','engine','--target-org','sandbox','--org-id',orgId],['local','--engine','engine','--target-org','sandbox']]) assert.throws(()=>parseOptions(args));
  const options=parseOptions(['run','--engine','engine','--target-org','sandbox','--org-id',orgId,'--allow-deploy','--sf-entry','run.js']);assert.equal(options.mode,'run');
});
test('fixture manifest conserves one explicit test identity and concrete negative expectations',()=>{
  assert.equal(validateManifest({schemaVersion:1,apiVersion:'68.0',scenarios:[scenario]}).scenarios.length,1);
  for(const invalid of [{...scenario,local:{...scenario.local,filter:'Probe.*'}},{...negative,expected:{...negative.expected,failure:undefined}},{...scenario,salesforce:{testClasses:['Different']}}]) assert.throws(()=>validateManifest({schemaVersion:1,apiVersion:'68.0',scenarios:[invalid]}));
  assert.throws(()=>validateManifest({schemaVersion:1,scenarios:[scenario]}));
});
test('every planned Salesforce deployment is check-only, explicit-target, explicit-tests and pinned API',()=>{
  const args=deployArguments(scenario,'selected org',7,'68.0');
  assert.ok(args.includes('--dry-run'));assert.equal(args[args.indexOf('--target-org')+1],'selected org');assert.equal(args[args.indexOf('--api-version')+1],'68.0');assert.equal(args[args.indexOf('--tests')+1],'Probe');
  assert.ok(!args.includes('--ignore-errors'));assert.ok(!args.includes('--ignore-warnings'));assert.ok(deployArguments(compiler,'sandbox',1,'68.0').includes('NoTestRun'));
  assert.throws(()=>deployArguments(scenario,'sandbox',1));
});
test('disposable identity accepts verified sandbox or unexpired scratch, rejects production and aliases alone',()=>{
  assert.equal(verifyOrgIdentity(org(),orgId).type,'sandbox');
  assert.equal(verifyOrgIdentity(org({TrialExpirationDate:'2099-01-01T00:00:00.000Z'}),orgId).type,'scratch');
  for(const value of [org({IsSandbox:false}),org({Id:'00D000000000002AAA'}),org({TrialExpirationDate:'2000-01-01'}),org({TrialExpirationDate:'invalid'}),{status:0,result:{records:org().result.records}}]) assert.throws(()=>verifyOrgIdentity(value,orgId),e=>e.code==='ORG_UNVERIFIED');
});
test('individual Salesforce test outcomes reconcile identities, terminal status and counts',()=>{
  assert.equal(salesforceObservation(oracle(),0,scenario).tests[0].status,'pass');
  const good=oracle(true);assert.equal(salesforceObservation(good,1,negative).tests[0].status,'fail');
  for(const mutate of [r=>{r.result.done=false;},r=>{r.result.checkOnly=false;},r=>{r.result.details.runTestResult.successes=[];},r=>{r.result.details.runTestResult.successes[0].methodName='other';},r=>{r.result.details.runTestResult.numTestsRun=true;},r=>{r.result.details.runTestResult.successes.push({...r.result.details.runTestResult.successes[0]});},r=>{r.status=1;},r=>{r.result.success=false;}]) {
    const r=oracle();mutate(r);assert.equal(salesforceObservation(r,0,scenario).compile,'unverified');
  }
});
test('auth, transport, pending deployment and coverage-only failures cannot match compiler negatives',()=>{
  for(const r of [{status:1,name:'AuthError',message:'String cannot be assigned to Integer'}, {...oracle(),result:{...oracle().result,status:'InProgress',done:false}}, {...oracle(true),result:{...oracle(true).result,details:{componentFailures:[]}}}]) assert.equal(salesforceObservation(r,1,compiler).compile,'unverified');
  const r=oracle(true);r.result.details={componentFailures:[{componentType:'ApexClass',fullName:'Bad',problemType:'Error',problem:'Illegal assignment from String to Integer',lineNumber:4}]};r.result.numberComponentErrors=1;
  const actual=salesforceObservation(r,1,compiler);assert.equal(actual.compile,'fail');assert.equal(satisfiesExpected(actual,compiler),'satisfied');
  r.result.details.componentFailures[0].fullName='CompletelyUnrelatedClass';assert.equal(salesforceObservation(r,1,compiler).compile,'unverified');
  r.result.details.componentFailures[0].fullName='Bad';r.result.details.componentFailures[0].lineNumber=42;assert.equal(salesforceObservation(r,1,compiler).compile,'unverified');
  r.result.details.componentFailures[0].componentType='Profile';assert.equal(salesforceObservation(r,1,compiler).compile,'unverified');
});
test('native blocked, zero-assertion, missing and extra tests remain unverified',()=>{
  assert.equal(nativeObservation(checked,execution(),scenario).tests[0].status,'pass');
  for(const modify of [r=>{r.status='blocked';},r=>{r.tests=[];},r=>{r.tests[0].id='Other.sample';},r=>{r.tests[0].assertions.executed=0;}]) {const r=execution();modify(r);assert.equal(nativeObservation(checked,r,scenario).compile,'unverified');}
  assert.equal(nativeObservation({status:'failed',exitCode:2,diagnostics:[{code:'APX_ENVIRONMENT',severity:'error',path:'Bad.cls',message:'String Integer'}]},undefined,compiler).compile,'unverified');
  assert.equal(nativeObservation({status:'failed',exitCode:2,diagnostics:[{code:'APX_TYPE_MISMATCH',severity:'error',path:path.resolve('fixture/force-app/Bad.cls'),message:'string integer',line:4}]},undefined,compiler,path.resolve('fixture')).compile,'fail');
});
test('a negative comparison requires the intended failure, not any failure or an absent org observation',()=>{
  const local=nativeObservation(checked,execution('failed'),negative);const remote=salesforceObservation(oracle(true),1,negative);
  assert.equal(compareObservations(local,remote,negative).verdict,'match');
  assert.equal(compareObservations(local,{compile:'unverified',tests:[]},negative).verdict,'unverified');
  const wrong=structuredClone(remote);wrong.tests[0].message='System.AssertException: failure';assert.equal(compareObservations(local,wrong,negative).verdict,'mismatch');
  assert.equal(compareObservations(nativeObservation(checked,execution(),scenario),salesforceObservation(oracle(),0,scenario),scenario).verdict,'match');
});
test('quoted or nested governor names cannot impersonate real limit failures',()=>{
  for(const message of ['System.AssertException: expected System.LimitException DML rows','System.DmlException: System.LimitException: Too many DML rows']) {
    const remote=oracle(true);remote.result.details.runTestResult.failures[0].message=message;
    assert.equal(satisfiesExpected(salesforceObservation(remote,1,negative),negative),'violated');
    const local=execution('failed');local.tests[0].message=message;local.diagnostics=[{severity:'error',code:'APX_EXCEPTION',message}];
    assert.equal(satisfiesExpected(nativeObservation(checked,local,negative),negative),'violated');
  }
  const local=execution('failed');local.diagnostics[0].code='APX_ASSERTION';assert.equal(satisfiesExpected(nativeObservation(checked,local,negative),negative),'violated');
  assert.throws(()=>validateManifest({schemaVersion:1,apiVersion:'68.0',scenarios:[{...negative,expected:{...negative.expected,failure:{category:'anything',messageIncludes:['DML rows']}}}]}));
});
test('native input manifests must refer to the original captured bytes and directory inventories',()=>{
  const project=path.resolve('snapshot');const files=[{path:'force-app/Probe.cls',sha256:'a'.repeat(64)}];const directories=new Map([[project,{normal:'b'.repeat(64),all:'c'.repeat(64)}]]);
  const report={inputs:[{path:path.join(project,files[0].path),sha256:files[0].sha256}],inputDirectories:[{path:project,sha256:'b'.repeat(64),includeIgnored:false}]};
  assert.doesNotThrow(()=>verifyCapturedInputs(report,project,files,directories));
  for(const mutate of [r=>{r.inputs[0].sha256='d'.repeat(64);},r=>{r.inputs[0].path=path.resolve('outside.cls');},r=>{r.inputs=[];},r=>{r.inputDirectories[0].includeIgnored=true;},r=>{r.inputDirectories[0].sha256='e'.repeat(64);}]) {const r=structuredClone(report);mutate(r);assert.throws(()=>verifyCapturedInputs(r,project,files,directories),e=>e.code==='INPUT_CHANGED');}
});
test('snapshot bounds actual bytes, includes empty directories, and honors cancellation',async t=>{
  const dir=await temporary(t);await fs.mkdir(path.join(dir,'empty'));const normal=await snapshot(dir);assert.deepEqual(normal.directories,['','empty']);
  const controller=new AbortController();controller.abort();await assert.rejects(snapshot(dir,{signal:controller.signal}),e=>e.code==='CANCELLED');
  const handle=await fs.open(path.join(dir,'large'),'w');await handle.truncate(32*1024*1024+1);await handle.close();
  await assert.rejects(snapshot(dir),e=>e.code==='FIXTURE');
});
test('evidence writer refuses overwrite and hashes the exact persisted bytes',async t=>{
  const dir=await temporary(t);await writeReceipt(dir,{verification:'unverified'});const before=await fs.readFile(path.join(dir,'receipt.json'));
  const digest=createHash('sha256').update(before).digest('hex');assert.match(await fs.readFile(path.join(dir,'receipt.sha256'),'utf8'),new RegExp(`^${digest}  receipt.json`));
  await assert.rejects(writeReceipt(dir,{verification:'matched'}),e=>e.code==='EEXIST');assert.deepEqual(await fs.readFile(path.join(dir,'receipt.json')),before);
});
test('process runner is bounded, shell-free, cancellation-aware and strips token environment',async()=>{
  process.env.APX_COMPARISON_SECRET='do-not-inherit';
  try {
    const r=await runProcess(process.execPath,['-e','process.stdout.write(JSON.stringify([process.argv[1],process.env.APX_COMPARISON_SECRET??null]))','literal & ; value']);assert.deepEqual(JSON.parse(r.stdout),['literal & ; value',null]);
    await assert.rejects(runProcess(process.execPath,['-e','setInterval(()=>{},1000)'],{timeoutMs:100}),e=>e.code==='TIMEOUT');
    await assert.rejects(runProcess(process.execPath,['-e',"process.stdout.write('x'.repeat(100000));setInterval(()=>{},1000)"],{maxBytes:100}),e=>e.code==='OUTPUT_LIMIT');
    const controller=new AbortController();controller.abort();await assert.rejects(runProcess(process.execPath,[],{signal:controller.signal}),e=>e.code==='CANCELLED');
  } finally {delete process.env.APX_COMPARISON_SECRET;}
});
test('receipt diagnostics redact common credentials',()=>{
  for(const secret of ['Bearer abcdef-secret','accessToken=abcdef-secret','refresh_token:abcdef-secret','force://id:abcdef-secret@example.invalid','00D000000000001AAA!abcdef-secret']) assert.doesNotMatch(redact(secret),/abcdef-secret/);
});
test('real fixture plan executes no engine or Salesforce CLI and preserves every selected scenario',async t=>{
  const dir=await temporary(t);const cli=path.join(root,'scripts/salesforce-compare.mjs');
  const result=spawnSync(process.execPath,[cli,'plan','--engine',path.join(dir,'missing-engine'),'--output',dir],{cwd:root,encoding:'utf8',timeout:30000,windowsHide:true});
  assert.equal(result.status,4,result.stderr);const output=JSON.parse(result.stdout);const receipt=JSON.parse(await fs.readFile(output.receipt,'utf8'));
  assert.equal(receipt.verification,'unverified');assert.equal(receipt.scenarios.length,10);assert.equal(receipt.selection.length,10);assert.ok(receipt.scenarios.every(s=>s.verdict==='unverified'&&!s.harnessError));assert.equal(receipt.org,undefined);assert.equal(receipt.engine,undefined);
});
test('orchestration refuses an unverified org before any deployment and records no credentials',async t=>{
  const dir=await temporary(t);const fake=path.join(dir,'sf-entry.mjs');const marker=path.join(dir,'deploy-was-attempted');
  await fs.writeFile(fake,`import fs from 'node:fs';const args=process.argv.slice(2);if(args[0]==='version'){console.log(JSON.stringify({cliVersion:'fixture CLI'}));}else if(args[0]==='org'||args[0]==='data'){console.log(JSON.stringify({status:1,name:'AuthError',accessToken:'must-not-be-recorded'}));process.exitCode=1;}else{fs.writeFileSync(${JSON.stringify(marker)},'unexpected');process.exitCode=1;}`);
  const result=spawnSync(process.execPath,[path.join(root,'scripts/salesforce-compare.mjs'),'run','--engine',process.execPath,'--sf-entry',fake,'--target-org','explicit-disposable','--org-id',orgId,'--allow-deploy','--output',dir],{cwd:root,encoding:'utf8',timeout:30000,windowsHide:true});
  assert.equal(result.status,4,result.stderr);const text=await fs.readFile(JSON.parse(result.stdout).receipt,'utf8');const receipt=JSON.parse(text);
  assert.equal(receipt.verification,'unverified');assert.equal(receipt.harnessError.code,'ORG_UNVERIFIED');assert.equal(receipt.scenarios.length,10);assert.ok(receipt.scenarios.every(s=>s.verdict==='unverified'));
  assert.doesNotMatch(text,/must-not-be-recorded|accessToken/);await assert.rejects(fs.access(marker),e=>e.code==='ENOENT');
});
test('native comparison records real fixture observations while leaving Salesforce unverified',{skip:!process.env.APX_ENGINE_PATH},async t=>{
  const dir=await temporary(t);const result=spawnSync(process.execPath,[path.join(root,'scripts/salesforce-compare.mjs'),'local','--engine',process.env.APX_ENGINE_PATH,'--output',dir],{cwd:root,encoding:'utf8',timeout:120000,windowsHide:true});
  assert.equal(result.status,4,result.stderr);const receipt=JSON.parse(await fs.readFile(JSON.parse(result.stdout).receipt,'utf8'));
  assert.equal(receipt.verification,'unverified');assert.equal(receipt.scenarios.length,10);assert.ok(receipt.scenarios.every(s=>s.localExpectation==='satisfied'&&s.verdict==='unverified'),JSON.stringify(receipt.scenarios.map(s=>({id:s.id,local:s.local,expectation:s.localExpectation,error:s.harnessError}))));
});
