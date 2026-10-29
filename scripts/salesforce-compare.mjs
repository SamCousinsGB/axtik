#!/usr/bin/env node
import fs from 'node:fs/promises';
import {createReadStream} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {createHash,randomUUID} from 'node:crypto';
import {spawn} from 'node:child_process';

const root=path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const hash=value=>createHash('sha256').update(value).digest('hex');
const object=value=>value!==null && typeof value==='object' && !Array.isArray(value);
const count=value=>(typeof value==='number'||typeof value==='string'&&/^\d+$/.test(value)) && Number.isSafeInteger(Number(value)) && Number(value)>=0;
const list=value=>value===undefined?[]:Array.isArray(value)?value:[value];
const idPattern=/^[A-Za-z][A-Za-z0-9_]*\.[A-Za-z][A-Za-z0-9_]*$/;
const orgPattern=/^00D[A-Za-z0-9]{12}(?:[A-Za-z0-9]{3})?$/;
const relative=file=>file.split(path.sep).join('/');
const contained=(base,file)=>{const rel=path.relative(base,file);return rel==='' || (!path.isAbsolute(rel) && rel!=='..' && !rel.startsWith(`..${path.sep}`));};
const issue=(code,message)=>Object.assign(new Error(message),{code});
const unverified=reason=>({compile:'unverified',tests:[],reason});

export function redact(text) {
  return String(text).replace(/(?:Bearer\s+)[A-Za-z0-9._~+/=-]+/gi,'Bearer [REDACTED]')
    .replace(/\b(?:access_token|accessToken|refresh_token|refreshToken|sfdxAuthUrl|password)\s*[=:]\s*[^\s,;]+/gi,'[REDACTED]')
    .replace(/force:\/\/[^\s]+/gi,'[REDACTED]').replace(/\b00D[A-Za-z0-9]{12}(?:[A-Za-z0-9]{3})?![A-Za-z0-9._~+/=-]+/g,'[REDACTED]')
    .slice(0,8192);
}

export function parseOptions(args) {
  const mode=args[0]??'plan';
  if(!['plan','local','run','--help'].includes(mode)) throw issue('ARGUMENT','Use plan, local, or run.');
  const options={mode,scenario:[]};
  for(let i=1;i<args.length;i++) {
    const key=args[i];
    if(key==='--allow-deploy') {options.allowDeploy=true;continue;}
    if(!['--manifest','--engine','--output','--target-org','--org-id','--sf-entry','--sf-executable','--timeout-ms','--wait-minutes','--scenario'].includes(key) || !args[i+1] || args[i+1].startsWith('--')) throw issue('ARGUMENT',`Unknown or incomplete option ${key}.`);
    const value=args[++i];
    if(key==='--scenario') options.scenario.push(value);
    else { const name=key.slice(2);if(options[name]!==undefined) throw issue('ARGUMENT',`Duplicate option ${key}.`);options[name]=value; }
  }
  options.timeoutMs=Number(options['timeout-ms']??120000);
  options.waitMinutes=Number(options['wait-minutes']??10);
  if(!Number.isSafeInteger(options.timeoutMs)||options.timeoutMs<100||options.timeoutMs>3600000) throw issue('ARGUMENT','--timeout-ms must be 100..3600000.');
  if(!Number.isSafeInteger(options.waitMinutes)||options.waitMinutes<1||options.waitMinutes>60) throw issue('ARGUMENT','--wait-minutes must be 1..60.');
  if(mode==='local' || mode==='run') { if(!options.engine) throw issue('ARGUMENT','--engine is required. The harness never downloads or builds an engine.'); }
  if(mode==='run') {
    if(!options['target-org'] || !orgPattern.test(options['org-id']??'') || !options.allowDeploy) throw issue('AUTHORIZATION','run requires --target-org, --org-id, and --allow-deploy. Only a check-only submission to a verified disposable org is permitted.');
    if(Boolean(options['sf-entry'])===Boolean(options['sf-executable'])) throw issue('ARGUMENT','Choose exactly one --sf-entry (SF CLI JavaScript entrypoint) or --sf-executable (native executable).');
  } else if(options.allowDeploy || options['target-org'] || options['org-id']) throw issue('ARGUMENT','Org arguments apply only to run. plan and local never contact Salesforce.');
  return options;
}

export function validateManifest(value) {
  if(!object(value)||value.schemaVersion!==1||typeof value.apiVersion!=='string'||!/^\d{2}\.0$/.test(value.apiVersion)||!Array.isArray(value.scenarios)||value.scenarios.length===0||value.scenarios.length>100) throw issue('MANIFEST','Invalid comparison manifest or missing pinned apiVersion.');
  const ids=new Set();
  for(const s of value.scenarios) {
    if(!object(s)||!/^[-a-z0-9]{1,80}$/.test(s.id)||ids.has(s.id)||typeof s.project!=='string'||!object(s.local)||!['check','test'].includes(s.local.command)||!Array.isArray(s.local.paths)||!s.local.paths.length||!s.local.paths.every(p=>typeof p==='string')||!object(s.salesforce)||!Array.isArray(s.salesforce.testClasses)||!s.salesforce.testClasses.every(c=>/^[A-Za-z][A-Za-z0-9_]*$/.test(c))||!object(s.expected)||!['pass','fail'].includes(s.expected.compile)||!['pass','fail','not_run'].includes(s.expected.tests)) throw issue('MANIFEST','Malformed or duplicate comparison scenario.');
    ids.add(s.id);
    if(s.local.environment!==undefined && typeof s.local.environment!=='string') throw issue('MANIFEST','Local environment must be a relative path.');
    if(s.local.command==='test' && (!idPattern.test(s.local.filter??'')||s.salesforce.testClasses.length!==1||s.salesforce.testClasses[0]!==s.local.filter.split('.')[0]||s.expected.compile!=='pass'||s.expected.tests==='not_run')) throw issue('MANIFEST','Test scenarios require one explicit Class.method and its Salesforce class.');
    if(s.local.command==='check' && (s.expected.tests!=='not_run'||s.salesforce.testClasses.length)) throw issue('MANIFEST','Compile scenarios cannot claim executed tests.');
    if(s.expected.compile==='fail'||s.expected.tests==='fail') {
      if(!object(s.expected.failure)||!['compile','governor','dml','assertion'].includes(s.expected.failure.category)||!Array.isArray(s.expected.failure.messageIncludes)||s.expected.failure.messageIncludes.length===0||s.expected.failure.messageIncludes.some(x=>typeof x!=='string'||x.length<2||x.length>256)) throw issue('MANIFEST','Expected failures require an explicit category and diagnostic message fragments.');
      if(s.expected.compile==='fail') {
        const location=s.expected.failure.location;
        if(s.expected.failure.category!=='compile'||!object(location)||typeof location.path!=='string'||path.isAbsolute(location.path)||!/^force-app\/.*\.(cls|trigger)$/.test(location.path)||location.path.split('/').includes('..')||!Number.isSafeInteger(location.line)||location.line<1) throw issue('MANIFEST','Compile-negative scenarios require an exact captured source path and line.');
      } else if(s.expected.failure.category==='compile') throw issue('MANIFEST','A runtime failure cannot use the compile category.');
    }
    if(s.requires!==undefined && (!Array.isArray(s.requires)||!s.requires.every(x=>typeof x==='string'&&x.length>0))) throw issue('MANIFEST','requires must be a list of preconditions.');
  }
  return value;
}

export function deployArguments(scenario,target,waitMinutes,apiVersion) {
  if(!/^\d{2}\.0$/.test(apiVersion??'')) throw issue('MANIFEST','Deployment requires an explicitly pinned API version.');
  const args=['project','deploy','start','--dry-run','--source-dir','force-app','--target-org',target,'--api-version',apiVersion,'--wait',String(waitMinutes),'--json'];
  if(scenario.local.command==='test') args.push('--test-level','RunSpecifiedTests','--tests',...scenario.salesforce.testClasses);
  else args.push('--test-level','NoTestRun');
  return args;
}

/** Live Organization data, rather than an alias or cached auth label, gates every submission. */
export function verifyOrgIdentity(envelope,expectedId,now=Date.now()) {
  if(!orgPattern.test(expectedId??'')) throw issue('ORG_UNVERIFIED','Expected org ID must be explicit.');
  const r=envelope?.result;
  if(envelope?.status!==0 || !r || r.done!==true || Number(r.totalSize)!==1 || !Array.isArray(r.records)||r.records.length!==1) throw issue('ORG_UNVERIFIED','The Organization identity query did not return exactly one complete record.');
  const org=r.records[0];
  if(!orgPattern.test(org.Id??'')||org.Id.slice(0,15)!==expectedId.slice(0,15)||org.IsSandbox!==true||!Object.hasOwn(org,'TrialExpirationDate')) throw issue('ORG_UNVERIFIED','Target identity is not the explicitly selected sandbox or scratch org.');
  const expiration=org.TrialExpirationDate;
  if(expiration!==null && (typeof expiration!=='string'||!Number.isFinite(Date.parse(expiration))||Date.parse(expiration)<=now)) throw issue('ORG_UNVERIFIED','Scratch org expiration could not be verified.');
  return {id:org.Id,type:expiration===null?'sandbox':'scratch',trialExpirationDate:expiration,verifiedAt:new Date(now).toISOString()};
}

const compileCodes=/^APX_(SYNTAX|SOURCE_UNIT|DUPLICATE_(METHOD|TYPE)|BIND_(NAME|TYPE|METHOD|MEMBER)|MEMBER_ACCESS|RETURN_TYPE|STATIC_CONTEXT|TYPE_MISMATCH)$/;
export function nativeObservation(check,execution,scenario,projectRoot) {
  if(check?.status==='failed' && check.exitCode===2 && check.diagnostics?.some(d=>d.severity==='error'&&compileCodes.test(d.code)&&/\.(cls|trigger)$/i.test(d.path))) {
    if(check.diagnostics.some(d=>d.severity==='error'&&!compileCodes.test(d.code))) return unverified('Native check includes non-compilation errors.');
    const diagnostics=check.diagnostics.filter(d=>d.severity==='error');
    const location=scenario.expected.failure?.location;
    if(location && (!projectRoot || diagnostics.some(d=>path.resolve(d.path)!==path.resolve(projectRoot,location.path)||d.line!==location.line))) return unverified('Native compiler diagnostics do not identify the expected captured source and line.');
    return {compile:'fail',failureCategory:'compile',tests:[],diagnostics:diagnostics.map(d=>({code:d.code,message:redact(d.message),line:d.line}))};
  }
  if(check?.status!=='checked'||check.exitCode!==0) return unverified('Native compilation did not produce a complete checked or compiler-error result.');
  if(scenario.local.command==='check') return {compile:'pass',tests:[]};
  if(!execution || !['preview_passed','failed'].includes(execution.status)||execution.exitCode!==(execution.status==='preview_passed'?4:1)||execution.tests?.length!==1||execution.tests[0].id!==scenario.local.filter||execution.tests[0].status!==(execution.status==='preview_passed'?'passed':'failed')) return unverified('Native execution is missing, blocked, errored, or has unexpected test identities.');
  const test=execution.tests[0];
  if(test.status==='passed' && !(test.assertions?.executed>0)) return unverified('Positive native test completed without an assertion.');
  const codes=(execution.diagnostics??[]).filter(d=>d.severity==='error'&&d.message===test.message).map(d=>d.code);
  const failureCategory=codes.includes('APX_GOVERNOR')&&/^(?:LIMIT_EXCEEDED:\s*)?System\.LimitException:/i.test(test.message)?'governor':codes.includes('APX_EXCEPTION')&&/^System\.DmlException:/i.test(test.message)?'dml':codes.includes('APX_ASSERTION')?'assertion':'unknown';
  return {compile:'pass',tests:[{id:test.id,status:test.status==='passed'?'pass':'fail',message:redact(test.message),failureCategory,diagnosticCodes:codes,assertions:test.assertions,setupAssertions:test.setupAssertions}],inputFingerprint:execution.inputFingerprint};
}

/** Only terminal Metadata API details can establish compile/test behavior. */
export function salesforceObservation(envelope,exitCode,scenario) {
  const result=envelope?.result;
  if(![0,1].includes(exitCode)||envelope?.status!==exitCode||!object(result)||result.checkOnly!==true||result.done!==true||!/^0Af[A-Za-z0-9]{12}(?:[A-Za-z0-9]{3})?$/.test(result.id??'')||!['Succeeded','Failed'].includes(result.status)||typeof result.success!=='boolean'||result.success!==(result.status==='Succeeded')||exitCode!==(result.success?0:1)) return unverified('No consistent terminal check-only Salesforce deployment result.');
  if(!count(result.numberComponentErrors)||!object(result.details)) return unverified('Missing Salesforce component accounting.');
  const errors=list(result.details.componentFailures).filter(d=>d.problemType==='Error');
  if(Number(result.numberComponentErrors)!==errors.length) return unverified('Salesforce component error counts do not reconcile.');
  if(errors.length) {
    if(result.success||errors.some(d=>!['ApexClass','ApexTrigger'].includes(d.componentType)||typeof d.problem!=='string'||!count(d.lineNumber)||Number(d.lineNumber)<1)) return unverified('Deployment errors are not exclusively identified Apex compiler diagnostics.');
    const location=scenario.expected.failure?.location;
    if(location) {
      const type=location.path.endsWith('.cls')?'ApexClass':'ApexTrigger';
      const component=path.posix.basename(location.path,path.posix.extname(location.path));
      if(errors.some(d=>d.fullName!==component||d.componentType!==type||Number(d.lineNumber)!==location.line)) return unverified('Salesforce compiler diagnostics do not identify the expected captured component and line.');
    }
    return {compile:'fail',failureCategory:'compile',tests:[],diagnostics:errors.map(d=>({component:d.fullName,message:redact(d.problem),line:Number(d.lineNumber)})),jobId:result.id};
  }
  if(scenario.local.command==='check') return result.success?{compile:'pass',tests:[],jobId:result.id}:unverified('Salesforce failed without compiler diagnostics.');
  const run=result.details.runTestResult;
  if(!object(run)||!count(run.numTestsRun)||!count(run.numFailures)) return unverified('Salesforce did not return test execution accounting.');
  const passed=list(run.successes),failed=list(run.failures);
  if(Number(run.numTestsRun)!==passed.length+failed.length||Number(run.numFailures)!==failed.length||passed.length+failed.length!==1||result.success!==(failed.length===0)) return unverified('Salesforce test counts or deployment outcome do not reconcile.');
  const tests=[...passed.map(t=>({...t,status:'pass'})),...failed.map(t=>({...t,status:'fail'}))];
  if(tests.some(t=>`${t.name}.${t.methodName}`!==scenario.local.filter||t.status==='fail'&&typeof t.message!=='string')) return unverified('Salesforce returned an unexpected or malformed test identity.');
  return {compile:'pass',tests:tests.map(t=>({id:`${t.name}.${t.methodName}`,status:t.status,message:redact(t.message??''),failureCategory:/^System\.LimitException:/i.test(t.message??'')?'governor':/^System\.DmlException:/i.test(t.message??'')?'dml':/^System\.AssertException:/i.test(t.message??'')?'assertion':'unknown'})),jobId:result.id};
}

export function satisfiesExpected(observation,scenario) {
  if(!observation||observation.compile==='unverified') return 'unverified';
  const expected=scenario.expected;
  if(observation.compile!==expected.compile) return 'violated';
  if(expected.compile==='pass' && expected.tests!=='not_run' && (observation.tests.length!==1||observation.tests[0].id!==scenario.local.filter||observation.tests[0].status!==expected.tests)) return 'violated';
  if(expected.compile==='fail'||expected.tests==='fail') {
    if((expected.compile==='fail'?observation.failureCategory:observation.tests[0].failureCategory)!==expected.failure.category) return 'violated';
    const messages=expected.compile==='fail'?observation.diagnostics.map(d=>d.message).join('\n'):observation.tests[0].message;
    if(!expected.failure.messageIncludes.every(fragment=>messages.toLowerCase().includes(fragment.toLowerCase()))) return 'violated';
  }
  return 'satisfied';
}

export function compareObservations(local,salesforce,scenario) {
  const localExpectation=satisfiesExpected(local,scenario),salesforceExpectation=satisfiesExpected(salesforce,scenario);
  return {verdict:localExpectation==='unverified'||salesforceExpectation==='unverified'?'unverified':localExpectation==='satisfied'&&salesforceExpectation==='satisfied'?'match':'mismatch',localExpectation,salesforceExpectation};
}

export function runProcess(command,args,{cwd,timeoutMs=120000,maxBytes=16*1024*1024,signal}={}) {
  return new Promise((resolve,reject)=>{
    if(signal?.aborted) return reject(issue('CANCELLED','Comparison cancelled.'));
    const env={};
    // SF authenticates from its standard credential store. Do not copy arbitrary tokens,
    // default-target variables, shell hooks, or Node injection flags into the child.
    for(const key of ['PATH','Path','HOME','USERPROFILE','APPDATA','LOCALAPPDATA','SystemRoot','WINDIR','TEMP','TMP','SSL_CERT_FILE','NODE_EXTRA_CA_CERTS']) if(process.env[key]) env[key]=process.env[key];
    Object.assign(env,{SF_DISABLE_TELEMETRY:'true',SFDX_DISABLE_TELEMETRY:'true',SF_AUTOUPDATE_DISABLE:'true',SF_DISABLE_LOG_FILE:'true',NO_COLOR:'1',CI:'1'});
    const child=spawn(command,args,{cwd,env,shell:false,windowsHide:true,stdio:['ignore','pipe','pipe']});
    const chunks=[];let bytes=0,error;
    const stop=code=>{if(!error) error=issue(code,`Child process ${code.toLowerCase()}.`);child.kill('SIGKILL');};
    const abort=()=>stop('CANCELLED');const timer=setTimeout(()=>stop('TIMEOUT'),timeoutMs);
    signal?.addEventListener('abort',abort,{once:true});if(signal?.aborted) abort();
    const cleanup=()=>{clearTimeout(timer);signal?.removeEventListener('abort',abort);};
    child.stdout.on('data',chunk=>{bytes+=chunk.length;if(bytes>maxBytes) stop('OUTPUT_LIMIT');else chunks.push(chunk);});
    child.stderr.on('data',chunk=>{bytes+=chunk.length;if(bytes>maxBytes) stop('OUTPUT_LIMIT');});
    child.on('error',()=>{cleanup();reject(error??issue('PROCESS_START','Cannot start the configured executable.'));});
    child.on('close',code=>{cleanup();if(error) reject(error);else if(code===null) reject(issue('PROCESS_EXIT','Child terminated without a process exit code.'));else resolve({exitCode:code,stdout:Buffer.concat(chunks).toString('utf8')});});
  });
}

async function digestFile(file,signal) { const digest=createHash('sha256');for await(const chunk of createReadStream(file,{signal})) digest.update(chunk);return digest.digest('hex'); }
async function readBounded(file,limit,signal) {
  const chunks=[];let bytes=0;
  // Inclusive end permits exactly one overflow byte; a concurrent grow cannot
  // cause an unbounded fs.readFile allocation.
  for await(const chunk of createReadStream(file,{end:limit,signal})) {
    bytes+=chunk.length;if(bytes>limit) throw issue('FIXTURE','Input exceeds its remaining byte bound.');chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}
async function safeChild(base,name) {
  if(typeof name!=='string'||path.isAbsolute(name)) throw issue('PATH','Fixture paths must be relative.');
  const resolved=await fs.realpath(path.resolve(base,name));
  if(!contained(base,resolved)) throw issue('PATH','Fixture path escapes its declared project or manifest directory.');
  return resolved;
}
export async function snapshot(project,{signal,timeoutMs=120000}={}) {
  const deadline=AbortSignal.timeout(timeoutMs);const boundedSignal=signal?AbortSignal.any([signal,deadline]):deadline;
  const files=[];files.directories=[''];let bytes=0,entries=0;
  async function visit(directory,depth=0) {
    if(boundedSignal.aborted) throw issue('CANCELLED','Fixture inventory cancelled or timed out.');
    if(depth>128) throw issue('FIXTURE','Fixture exceeds the directory depth bound.');
    for await(const entry of await fs.opendir(directory)) {
      if(boundedSignal.aborted) throw issue('CANCELLED','Fixture inventory cancelled or timed out.');
      if(++entries>10000) throw issue('FIXTURE','Fixture exceeds 10000 inventory entries.');
      if(['.git','.sf','.sfdx','node_modules'].includes(entry.name)) continue;
      const file=path.join(directory,entry.name);
      if(entry.isSymbolicLink()) throw issue('FIXTURE','Fixture symlinks are not supported.');
      if(entry.isDirectory()) {files.directories.push(relative(path.relative(project,file)));await visit(file,depth+1);}
      else if(entry.isFile()) {
        const content=await readBounded(file,32*1024*1024-bytes,boundedSignal);bytes+=content.length;
        files.push({path:relative(path.relative(project,file)),sha256:hash(content),content});
      } else throw issue('FIXTURE','Fixture contains a special filesystem entry.');
    }
  }
  await visit(project);files.sort((a,b)=>Buffer.compare(Buffer.from(a.path),Buffer.from(b.path)));files.directories.sort((a,b)=>Buffer.compare(Buffer.from(a),Buffer.from(b)));
  return files;
}
const fileIdentity=files=>files.map(({path,sha256})=>({path,sha256}));
const snapshotIdentity=files=>({files:fileIdentity(files),directories:files.directories});
export function verifyCapturedInputs(report,project,files,directories) {
  const expectedFiles=new Map(files.map(f=>[path.resolve(project,f.path),f.sha256]));
  if(report.inputs.some(input=>expectedFiles.get(path.resolve(input.path))!==input.sha256)) throw issue('INPUT_CHANGED','Native input bytes differ from the captured fixture.');
  for(const input of report.inputDirectories) {
    const baseline=directories.get(path.resolve(input.path));
    if(!baseline||input.sha256!==baseline[input.includeIgnored?'all':'normal']) throw issue('INPUT_CHANGED','Native directory inventory differs from the captured fixture.');
  }
  for(const file of files.filter(file=>/\.(cls|trigger)$/i.test(file.path)||file.path==='sfdx-project.json')) {
    if(!report.inputs.some(input=>path.resolve(input.path)===path.resolve(project,file.path))) throw issue('INPUT_CHANGED','Native report omitted a captured Apex or project input.');
  }
}
export async function writeReceipt(directory,receipt) {
  const json=JSON.stringify(receipt,null,2)+'\n';
  await fs.writeFile(path.join(directory,'receipt.json'),json,{flag:'wx'});
  await fs.writeFile(path.join(directory,'receipt.sha256'),hash(json)+'  receipt.json\n',{flag:'wx'});
}

export async function main(args=process.argv.slice(2)) {
  const options=parseOptions(args);
  if(options.mode==='--help') { console.log('node scripts/salesforce-compare.mjs plan|local|run [--manifest file] [--engine absolute-path] [--output directory] [--scenario id]\nrun additionally requires --target-org alias --org-id 00D... --allow-deploy and --sf-entry run.js (or --sf-executable path). All Salesforce submissions use --dry-run.');return 0; }
  const manifestPath=await fs.realpath(path.resolve(options.manifest??path.join(root,'corpus/salesforce-comparison/manifest.json')));
  const manifestBytes=await readBounded(manifestPath,1024*1024,AbortSignal.timeout(options.timeoutMs));
  const manifest=validateManifest(JSON.parse(manifestBytes));
  const scenarios=manifest.scenarios.filter(s=>!options.scenario.length||options.scenario.includes(s.id));
  if(!scenarios.length||options.scenario.some(id=>!scenarios.some(s=>s.id===id))) throw issue('ARGUMENT','A selected scenario does not exist.');
  const requestedOutput=path.resolve(options.output??path.join(root,'.tmp/salesforce-comparison'));
  await fs.mkdir(requestedOutput,{recursive:true});
  // The native engine reports canonical paths. Use the same filesystem identity
  // for captured inputs, including macOS /var -> /private/var and user aliases.
  const output=await fs.realpath(requestedOutput);
  const directory=path.join(output,`${new Date().toISOString().replaceAll(':','-')}-${randomUUID()}`);await fs.mkdir(directory);
  const receipt={schemaVersion:1,mode:options.mode,apiVersion:manifest.apiVersion,selection:scenarios.map(s=>s.id),createdAt:new Date().toISOString(),verification:'unverified',manifestSha256:hash(manifestBytes),qualification:'A fixture match is scoped evidence, not Salesforce certification.',scenarios:[]};
  const controller=new AbortController();const cancel=()=>controller.abort();process.once('SIGINT',cancel);process.once('SIGTERM',cancel);
  try {
    let engine,runEngine,directoryFingerprint,sf,canonicalTarget;
    if(options.mode!=='plan') {
      engine=await fs.realpath(path.resolve(options.engine));receipt.engine={sha256:await digestFile(engine,AbortSignal.any([controller.signal,AbortSignal.timeout(options.timeoutMs)]))};
      ({runEngine,directoryFingerprint}=await import('../packages/runner/lib/index.js'));
    }
    if(options.mode==='run') {
      const executable=await fs.realpath(path.resolve(options['sf-entry']??options['sf-executable']));
      if(!options['sf-entry'] && /\.(cmd|bat|ps1)$/i.test(executable)) throw issue('ARGUMENT','Shell launchers are not executed. Use --sf-entry with the installed SF CLI bin/run.js.');
      sf=argv=>runProcess(options['sf-entry']?process.execPath:executable,options['sf-entry']?[executable,...argv]:argv,{cwd:directory,timeoutMs:options.timeoutMs,signal:controller.signal});
      const version=await sf(['version','--json']);
      if(version.exitCode!==0) throw issue('CLI_VERSION','Could not identify the Salesforce CLI.');
      const versionJson=JSON.parse(version.stdout);const versionText=versionJson?.result?.cliVersion??versionJson?.cliVersion;
      if(typeof versionText!=='string') throw issue('CLI_VERSION','Salesforce CLI version JSON is incompatible.');
      receipt.salesforceCli={entrySha256:await digestFile(executable),version:redact(versionText),node:process.version};
      const display=await sf(['org','display','--target-org',options['target-org'],'--json']);
      const resolved=JSON.parse(display.stdout);
      if(display.exitCode!==0||resolved.status!==0||!orgPattern.test(resolved.result?.id??'')||resolved.result.id.slice(0,15)!==options['org-id'].slice(0,15)||typeof resolved.result.username!=='string'||!resolved.result.username.includes('@')||resolved.result.username.startsWith('-')) throw issue('ORG_UNVERIFIED','Canonical org username could not be resolved.');
      canonicalTarget=resolved.result.username;
      // org display can contain credentials. Keep only a username hash and never
      // persist its raw stdout, token fields, or auth URLs.
      receipt.resolvedUsernameSha256=hash(canonicalTarget);
      const identity=await sf(['data','query','--query','SELECT Id, IsSandbox, TrialExpirationDate FROM Organization','--target-org',canonicalTarget,'--json']);
      if(identity.exitCode!==0) throw issue('ORG_UNVERIFIED','Organization identity query failed.');
      receipt.org=verifyOrgIdentity(JSON.parse(identity.stdout),options['org-id']);
    }
    for(const scenario of scenarios) {
      const item={id:scenario.id,expected:scenario.expected,requires:scenario.requires??[],local:unverified('Not executed.'),salesforce:unverified('No Salesforce observation.'),verdict:'unverified'};receipt.scenarios.push(item);
      try {
        const project=await safeChild(path.dirname(manifestPath),scenario.project);
        if(contained(project,output)) throw issue('PATH','Evidence output must be outside every source fixture project.');
        const snapshotOptions={signal:controller.signal,timeoutMs:options.timeoutMs};
        const files=await snapshot(project,snapshotOptions);const again=await snapshot(project,snapshotOptions);
        if(hash(JSON.stringify(snapshotIdentity(files)))!==hash(JSON.stringify(snapshotIdentity(again)))) throw issue('INPUT_CHANGED','Fixture changed while being captured.');
        item.files=fileIdentity(files);item.directories=files.directories;item.sourceFingerprint=hash(JSON.stringify(snapshotIdentity(files)));
        for(const name of [...scenario.local.paths,...(scenario.local.environment?[scenario.local.environment]:[])]) await safeChild(project,name);
        await safeChild(project,'force-app');await safeChild(project,'sfdx-project.json');
        if(JSON.parse(files.find(f=>f.path==='sfdx-project.json')?.content??'{}').sourceApiVersion!==manifest.apiVersion) throw issue('API_VERSION','Fixture sourceApiVersion differs from the manifest transport version.');
        const nativeArgs=command=>[command,'--project','${PROJECT}','--json',...(command==='test'?['--filter',scenario.local.filter]:[]),...(scenario.local.environment?['--environment',scenario.local.environment]:[]),...scenario.local.paths];
        item.commands={native:[nativeArgs('check'),...(scenario.local.command==='test'?[nativeArgs('test')]:[])],salesforce:deployArguments(scenario,'${TARGET_ORG}',options.waitMinutes,manifest.apiVersion)};
        if(options.mode==='plan') { item.local.reason='Plan only; no process executed.';continue; }
        const copy=path.join(directory,'projects',scenario.id);await fs.mkdir(copy,{recursive:true});
        for(const folder of files.directories) await fs.mkdir(path.join(copy,folder),{recursive:true});
        for(const file of files) {const target=path.join(copy,file.path);await fs.mkdir(path.dirname(target),{recursive:true});await fs.writeFile(target,file.content,{flag:'wx'});}
        const directories=new Map();
        const captureSignal=AbortSignal.any([controller.signal,AbortSignal.timeout(options.timeoutMs)]);
        for(const folder of files.directories) {
          const name=path.join(copy,folder);
          directories.set(path.resolve(name),{normal:(await directoryFingerprint(name,10000,captureSignal,false)).sha256,all:(await directoryFingerprint(name,10000,captureSignal,true)).sha256});
        }
        const run=command=>runEngine({enginePath:engine,command,projectRoot:copy,paths:scenario.local.paths,filter:command==='test'?scenario.local.filter:undefined,environmentPath:scenario.local.environment?path.join(copy,scenario.local.environment):undefined,timeoutMs:options.timeoutMs,signal:controller.signal});
        const check=await run('check');verifyCapturedInputs(check,copy,files,directories);
        const execution=check.status==='checked'&&scenario.local.command==='test'?await run('test'):undefined;
        if(execution) {verifyCapturedInputs(execution,copy,files,directories);if(check.inputFingerprint!==execution.inputFingerprint) throw issue('INPUT_CHANGED','Compilation and execution used different input snapshots.');}
        item.local=nativeObservation(check,execution,scenario,copy);item.local.inputFingerprint=(execution??check).inputFingerprint;
        if(options.mode==='run' && !item.requires.length) {
          const identity=await sf(['data','query','--query','SELECT Id, IsSandbox, TrialExpirationDate FROM Organization','--target-org',canonicalTarget,'--json']);
          if(identity.exitCode!==0) throw issue('ORG_UNVERIFIED','Target identity could not be reverified before submission.');
          verifyOrgIdentity(JSON.parse(identity.stdout),receipt.org.id);
          const args=deployArguments(scenario,canonicalTarget,options.waitMinutes,manifest.apiVersion);
          // Pin the resolved username, then reverify its live ID before every call.
          const processResult=await runProcess(options['sf-entry']?process.execPath:await fs.realpath(options['sf-executable']),options['sf-entry']?[await fs.realpath(options['sf-entry']),...args]:args,{cwd:copy,timeoutMs:options.timeoutMs,signal:controller.signal});
          item.salesforce=salesforceObservation(JSON.parse(processResult.stdout),processResult.exitCode,scenario);
        } else if(item.requires.length) item.salesforce=unverified('Scenario has controlled-environment prerequisites without executable verification. No Salesforce submission made.');
        const after=await snapshot(copy,snapshotOptions);
        if(hash(JSON.stringify(snapshotIdentity(after)))!==item.sourceFingerprint) throw issue('INPUT_CHANGED','Fixture changed during execution.');
        if(await digestFile(engine,AbortSignal.any([controller.signal,AbortSignal.timeout(options.timeoutMs)]))!==receipt.engine.sha256) throw issue('ENGINE_CHANGED','Native engine changed during execution.');
        Object.assign(item,compareObservations(item.local,item.salesforce,scenario));
      } catch(error) {item.verdict='unverified';item.harnessError={code:typeof error.code==='string'?error.code:'HARNESS_ERROR'};}
    }
    receipt.verification=receipt.scenarios.length===scenarios.length&&receipt.scenarios.every(s=>s.verdict==='match')?'matched':receipt.scenarios.some(s=>s.verdict==='mismatch')?'mismatch':'unverified';
  } catch(error) {
    receipt.harnessError={code:typeof error.code==='string'?error.code:'HARNESS_ERROR'};receipt.verification='unverified';
    for(const scenario of scenarios) if(!receipt.scenarios.some(s=>s.id===scenario.id)) receipt.scenarios.push({id:scenario.id,verdict:'unverified',local:unverified('Harness stopped before this scenario.'),salesforce:unverified('Harness stopped before this scenario.')});
  }
  finally {process.removeListener('SIGINT',cancel);process.removeListener('SIGTERM',cancel);receipt.completedAt=new Date().toISOString();await writeReceipt(directory,receipt);}
  console.log(JSON.stringify({verification:receipt.verification,receipt:path.join(directory,'receipt.json'),scenarios:receipt.scenarios.map(s=>({id:s.id,verdict:s.verdict,localExpectation:s.localExpectation})),harnessError:receipt.harnessError},null,2));
  return receipt.verification==='matched'?0:receipt.verification==='mismatch'?1:4;
}

if(process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) main().then(code=>{process.exitCode=code;},error=>{console.error(`${error.code??'HARNESS_ERROR'}: ${redact(error.message)}`);process.exitCode=2;});
