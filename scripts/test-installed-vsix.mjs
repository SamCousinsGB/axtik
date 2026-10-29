#!/usr/bin/env node
// Installs the actual VSIX into an isolated VS Code profile, then runs the real
// Extension Host suite. Downloading VS Code is a build/test operation only.
import fs from 'node:fs/promises';
import {createReadStream} from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import {fileURLToPath} from 'node:url';
import {createHash,randomUUID} from 'node:crypto';
import {spawn} from 'node:child_process';

const filename=fileURLToPath(import.meta.url);
const root=path.dirname(path.dirname(filename));
const issue=(code,message)=>Object.assign(new Error(message),{code});
const within=(base,file)=>{const relative=path.relative(base,file);return relative===''||(!path.isAbsolute(relative)&&relative!=='..'&&!relative.startsWith(`..${path.sep}`));};
const sha=async file=>{const digest=createHash('sha256');for await(const chunk of createReadStream(file)) digest.update(chunk);return digest.digest('hex');};
const json=async file=>JSON.parse(await fs.readFile(file,'utf8'));

export const HOST_CHECKS=[
  'commands registered in real Extension Host',
  'class command calls native engine and publishes Problems',
  'repaired source clears stale Problems',
  'folder command uses the same native pipeline',
  'editing source invalidates applied diagnostics',
  'bundled engine discovers demo tests through the extension command',
  'production class validation expands selection and preserves advisory exit 4',
  'project-relative environment setting reaches bundled native evaluator',
  'environment edits invalidate applied diagnostics',
  'environment setting changes invalidate applied diagnostics',
  'real native manifest includes binary and expanded resource inputs',
  'completed external result invalidates on binary png payload edit',
  'completed external result invalidates on expanded js payload edit',
  'completed external result invalidates on expanded csv payload edit',
  'completed external result invalidates on resource payload rename',
  'completed external result invalidates on resource payload deletion',
  'expanded resource additions inside normally ignored dist invalidate results',
  'unsaved expanded resource edit invalidates applied diagnostics',
  'completed result retains watcher for environment outside project',
  'actual assertion invocations distinguish helpers, setup and zero assertion execution',
  'unknown native API in unexecuted source blocks validation',
];

export function parseOptions(args) {
  const result={timeoutMs:180000,downloadTimeoutMs:600000,vscodeVersion:'stable',output:path.join(root,'.tmp','installed-vsix')};
  for(let i=0;i<args.length;i++) {
    const key=args[i];
    if(key==='--help') {result.help=true;continue;}
    if(!['--vsix','--engine','--output','--vscode-path','--vscode-version','--timeout-ms','--download-timeout-ms'].includes(key)||!args[i+1]||args[i+1].startsWith('--')) throw issue('ARGUMENT',`Unknown or incomplete option ${key}`);
    const name={'--vscode-path':'vscodePath','--vscode-version':'vscodeVersion','--timeout-ms':'timeoutMs','--download-timeout-ms':'downloadTimeoutMs'}[key]??key.slice(2);
    if(Object.hasOwn(result,`seen:${name}`)) throw issue('ARGUMENT',`Duplicate option ${key}`);
    result[`seen:${name}`]=true;result[name]=args[++i];
  }
  if(result.help) return result;
  if(!result.vsix||!result.engine) throw issue('ARGUMENT','--vsix and --engine are required');
  for(const key of ['timeoutMs','downloadTimeoutMs']) {
    result[key]=Number(result[key]);
    if(!Number.isSafeInteger(result[key])||result[key]<100||result[key]>1200000) throw issue('ARGUMENT',`${key} must be 100..1200000`);
  }
  if(!/^(stable|insiders|\d+\.\d+\.\d+)$/.test(result.vscodeVersion)) throw issue('ARGUMENT','Use a VS Code version such as 1.90.0, stable, or insiders');
  return result;
}

export function childEnvironment(extra={}) {
  const env={};
  for(const key of ['SystemRoot','WINDIR','TEMP','TMP','TMPDIR','HOME','USERPROFILE','PATH','PATHEXT','DISPLAY','WAYLAND_DISPLAY','XAUTHORITY','XDG_RUNTIME_DIR','DBUS_SESSION_BUS_ADDRESS','LANG','LC_ALL','HTTP_PROXY','HTTPS_PROXY','NO_PROXY','http_proxy','https_proxy','no_proxy','NODE_EXTRA_CA_CERTS','SSL_CERT_FILE','SSL_CERT_DIR']) if(process.env[key]) env[key]=process.env[key];
  return {...env,...extra};
}

/** macOS IPC sockets have a 103-byte path limit, including VS Code's suffix. */
export async function createUserDataProfile(state,{platform=process.platform,temporaryDirectory=os.tmpdir(),fallbackDirectory='/tmp'}={}) {
  if(platform==='win32') return {directory:path.join(state,'user'),temporary:false,cleanup:async()=>{}};
  // Keep at least 43 bytes for the editor's socket filename. Canonicalization
  // happens first because /tmp and /var can expand to /private/... on macOS.
  const maxPathBytes=60,prefix='apx-vsc-',attempted=new Set();
  for(const candidate of [temporaryDirectory,fallbackDirectory]) {
    let parent;
    try {parent=await fs.realpath(candidate);} catch(error) {if(['ENOENT','EACCES','EPERM'].includes(error.code)) continue;throw error;}
    if(attempted.has(parent)) continue;attempted.add(parent);
    if(Buffer.byteLength(path.join(parent,`${prefix}XXXXXX`))>maxPathBytes) continue;
    let directory;
    try {directory=await fs.mkdtemp(path.join(parent,prefix));} catch(error) {if(['EACCES','EPERM'].includes(error.code)) continue;throw error;}
    directory=await fs.realpath(directory);
    await fs.chmod(directory,0o700);
    const identity=await fs.lstat(directory);
    let cleaned=false;
    const cleanup=async()=>{
      if(cleaned) return;
      const current=await fs.lstat(directory);
      if(path.dirname(directory)!==parent||!/^apx-vsc-[A-Za-z0-9]{6}$/.test(path.basename(directory))||current.isSymbolicLink()||!current.isDirectory()||current.dev!==identity.dev||current.ino!==identity.ino||await fs.realpath(directory)!==directory) throw issue('PROFILE_CLEANUP','Refusing to remove a replaced or unowned VS Code profile');
      await fs.rm(directory,{recursive:true,force:true});cleaned=true;
    };
    return {directory,temporary:true,cleanup};
  }
  throw issue('PROFILE_PATH','No writable temporary directory can keep VS Code IPC socket paths short enough');
}

/** Bound output and elapsed time; kill the spawned editor's process tree on failure. */
export function runProcess(executable,args,{cwd=root,env=childEnvironment(),timeoutMs=180000,maxOutputBytes=16*1024*1024,signal}={}) {
  if(signal?.aborted) return Promise.reject(issue('CANCELLED','Test cancelled before process launch'));
  return new Promise((resolve,reject)=>{
    const child=spawn(executable,args,{cwd,env,shell:false,windowsHide:true,detached:process.platform!=='win32',stdio:['ignore','pipe','pipe']});
    const stdout=[],stderr=[];let bytes=0,failure;
    const kill=()=>{
      if(!child.pid) return;
      if(process.platform==='win32') {
        const taskkill=path.join(process.env.SystemRoot??'C:\\Windows','System32','taskkill.exe');
        const helper=spawn(taskkill,['/PID',String(child.pid),'/T','/F'],{shell:false,windowsHide:true,stdio:'ignore'});
        helper.on('error',()=>child.kill('SIGKILL'));
      } else {try {process.kill(-child.pid,'SIGKILL');} catch {child.kill('SIGKILL');}}
    };
    const stop=(code,message)=>{if(failure) return;failure=issue(code,message);kill();};
    const abort=()=>stop('CANCELLED','Installed extension test cancelled');
    const timer=setTimeout(()=>stop('TIMEOUT','Installed extension test exceeded its deadline'),timeoutMs);
    signal?.addEventListener('abort',abort,{once:true});if(signal?.aborted) abort();
    const cleanup=()=>{clearTimeout(timer);signal?.removeEventListener('abort',abort);};
    const captured=()=>({stdout:Buffer.concat(stdout).toString('utf8'),stderr:Buffer.concat(stderr).toString('utf8')});
    for(const [stream,chunks] of [[child.stdout,stdout],[child.stderr,stderr]]) stream.on('data',chunk=>{
      bytes+=chunk.length;
      if(bytes>maxOutputBytes) stop('OUTPUT_LIMIT','Installed extension test exceeded its output bound');
      else chunks.push(chunk);
    });
    child.once('error',error=>{cleanup();reject(Object.assign(failure??issue('PROCESS_START',error.message),captured()));});
    child.once('close',code=>{
      cleanup();
      if(failure) reject(Object.assign(failure,captured()));
      else if(code===null) reject(issue('PROCESS_EXIT','Child exited without an exit code'));
      else resolve({exitCode:code,...captured()});
    });
  });
}

/** Locate VS Code's JS CLI without ever executing a .cmd/.sh shell launcher. */
export async function locateVSCode(value,platform=process.platform) {
  let executable=path.resolve(value);
  if((await fs.stat(executable)).isDirectory()) {
    if(platform==='darwin') executable=path.join(executable,'Contents','MacOS','Electron');
    else executable=path.join(executable,platform==='win32'?'Code.exe':'code');
  }
  executable=await fs.realpath(executable);
  const base=path.dirname(executable);
  const candidates=platform==='darwin'?[path.resolve(base,'../Resources/app/out/cli.js')]:[path.join(base,'resources','app','out','cli.js')];
  if(platform==='win32') {
    // Current Windows distributions can retain an update-specific application
    // directory. Read only the CLI path literal from Microsoft's launcher.
    const launcher=await fs.readFile(path.join(base,'bin','code.cmd'),'utf8').catch(()=>null);
    if(launcher) {
      const matches=[...launcher.matchAll(/"%~dp0\.\.[\\/]([^"\r\n]*resources[\\/]app[\\/]out[\\/]cli\.js)"/gi)];
      for(const match of matches) {
        const file=path.resolve(base,...match[1].split(/[\\/]/));
        if(!within(base,file)) throw issue('VSCODE_LAYOUT','VS Code CLI launcher points outside its installation');
        candidates.unshift(file);
      }
    }
  }
  for(const candidate of candidates) {
    try {
      const cli=await fs.realpath(candidate);
      const installRoot=platform==='darwin'?path.resolve(base,'..'):base;
      if(!within(installRoot,cli)) throw issue('VSCODE_LAYOUT','VS Code CLI escaped its installation');
      const app=path.resolve(path.dirname(cli),'..');
      const manifest=await json(path.join(app,'package.json'));
      if(typeof manifest.version!=='string') throw issue('VSCODE_LAYOUT','VS Code application version is missing');
      return {executable,cli,version:manifest.version};
    } catch(error) {if(error.code!=='ENOENT') throw error;}
  }
  throw issue('VSCODE_LAYOUT','Cannot locate resources/app/out/cli.js in this VS Code installation');
}

export async function findInstalledExtension(extensions,id,version) {
  const candidates=[];
  for(const entry of await fs.readdir(extensions,{withFileTypes:true})) {
    if(!entry.isDirectory()||entry.isSymbolicLink()) continue;
    const folder=path.join(extensions,entry.name);
    let manifest;try {manifest=await json(path.join(folder,'package.json'));} catch(error) {if(error.code==='ENOENT') continue;throw error;}
    if(`${manifest.publisher}.${manifest.name}`===id&&manifest.version===version) candidates.push(await fs.realpath(folder));
  }
  if(candidates.length!==1) throw issue('EXTENSION_INSTALL',`Expected exactly one installed ${id}@${version}; found ${candidates.length}`);
  return candidates[0];
}

export async function verifyHostReport(report,{extensionRoot,extensionId,version,engineSha256,vscodeVersion,platform=process.platform,arch=process.arch}) {
  if(report?.schemaVersion!==1||!Array.isArray(report.results)||report.results.length!==HOST_CHECKS.length||new Set(report.results.map(r=>r?.name)).size!==HOST_CHECKS.length||report.results.some(r=>r?.passed!==true)||HOST_CHECKS.some(name=>!report.results.some(r=>r.name===name))) throw issue('HOST_EVIDENCE','Installed host evidence must contain every unique expected passing check');
  if(report.engineSha256!==engineSha256||report.platform!==platform||report.arch!==arch||report.extensionId!==extensionId||report.version!==version||report.vscodeVersion!==vscodeVersion) throw issue('HOST_EVIDENCE','Installed host evidence belongs to a different package, binary or platform');
  if(typeof report.extensionRoot!=='string'||await fs.realpath(report.extensionRoot)!==await fs.realpath(extensionRoot)) throw issue('HOST_EVIDENCE','Host loaded a development copy or a different installed extension');
}

async function download(options,state,signal) {
  const config=path.join(state,'download-options.json'),result=path.join(state,'download-result.json');
  await fs.writeFile(config,JSON.stringify({version:options.vscodeVersion,cachePath:path.join(state,'vscode-cache'),timeout:30000,result}));
  const child=await runProcess(process.execPath,[filename,'--internal-download',config],{timeoutMs:options.downloadTimeoutMs,signal});
  await fs.writeFile(path.join(state,'download.stdout.log'),child.stdout);await fs.writeFile(path.join(state,'download.stderr.log'),child.stderr);
  if(child.exitCode!==0) throw issue('VSCODE_DOWNLOAD',`VS Code download exited ${child.exitCode}; inspect download logs`);
  const response=await json(result);if(typeof response.executable!=='string') throw issue('VSCODE_DOWNLOAD','Download did not identify an executable');
  return response.executable;
}

export async function main(args=process.argv.slice(2)) {
  const options=parseOptions(args);
  if(options.help) {console.log('node scripts/test-installed-vsix.mjs --vsix file.vsix --engine native-binary [--output directory] [--vscode-path executable | --vscode-version 1.90.0] [--timeout-ms 180000] [--download-timeout-ms 600000]\nWithout --vscode-path, explicitly running this test downloads VS Code. Linux desktop tests require a display (use xvfb-run in CI).');return 0;}
  const vsix=await fs.realpath(options.vsix),engine=await fs.realpath(options.engine);
  const output=path.resolve(options.output);await fs.mkdir(output,{recursive:true});
  const state=await fs.realpath(await fs.mkdtemp(path.join(output,`${new Date().toISOString().replaceAll(':','-')}-${randomUUID()}-`)));
  const report={schemaVersion:1,kind:'installed-vsix',verified:false,createdAt:new Date().toISOString(),target:`${process.platform}-${process.arch}`,platform:process.platform,arch:process.arch,vsixSha256:await sha(vsix),engineSha256:await sha(engine),state};
  const controller=new AbortController();const cancel=()=>controller.abort();process.once('SIGINT',cancel);process.once('SIGTERM',cancel);
  let profile;
  try {
    const manifest=await json(path.join(root,'packages','vscode','package.json'));
    const id=`${manifest.publisher}.${manifest.name}`;report.extensionId=id;report.version=manifest.version;
    const version=await runProcess(engine,['--version'],{timeoutMs:10000,signal:controller.signal});
    if(version.exitCode!==0||version.stdout.trim()!==`apx ${manifest.version}`) throw issue('ENGINE_VERSION','Frozen native engine and extension manifest versions differ');
    const vscode=await locateVSCode(options.vscodePath??await download(options,state,controller.signal));report.vscodeVersion=vscode.version;
    profile=await createUserDataProfile(state);report.userDataDirectory=profile.directory;
    const extensions=path.join(state,'extensions'),userData=profile.directory;
    await fs.mkdir(extensions);await fs.mkdir(path.join(userData,'User'),{recursive:true});
    await fs.writeFile(path.join(userData,'User','settings.json'),JSON.stringify({'telemetry.telemetryLevel':'off','update.mode':'none','extensions.autoCheckUpdates':false,'extensions.autoUpdate':false}));
    const install=await runProcess(vscode.executable,[vscode.cli,'--install-extension',vsix,'--extensions-dir',extensions,'--user-data-dir',userData,'--force','--disable-telemetry'],{env:childEnvironment({ELECTRON_RUN_AS_NODE:'1'}),timeoutMs:options.timeoutMs,signal:controller.signal});
    await fs.writeFile(path.join(state,'install.stdout.log'),install.stdout);await fs.writeFile(path.join(state,'install.stderr.log'),install.stderr);report.installExitCode=install.exitCode;
    if(install.exitCode!==0) throw issue('EXTENSION_INSTALL',`VSIX installation exited ${install.exitCode}; inspect install logs`);
    const extensionRoot=await findInstalledExtension(extensions,id,manifest.version);report.extensionRoot=extensionRoot;
    const hostReportPath=path.join(state,'host-report.json');
    const env=childEnvironment({APX_HOST_TEST_REPORT:hostReportPath,APX_HOST_DEMO_PROJECT:await fs.realpath(path.join(root,'fixtures','demo')),APX_HOST_EXPECT_EXTENSION_ROOT:extensionRoot,APX_HOST_EXPECT_EXTENSION_ID:id,APX_HOST_EXPECT_ENGINE_SHA256:report.engineSha256});
    const host=await runProcess(vscode.executable,[`--extensionDevelopmentPath=${path.join(root,'packages','vscode','test','driver')}`,`--extensionTestsPath=${path.join(root,'packages','vscode','test','host.cjs')}`,`--user-data-dir=${userData}`,`--extensions-dir=${extensions}`,'--disable-workspace-trust','--skip-welcome','--disable-updates','--disable-telemetry','--no-sandbox'],{env,timeoutMs:options.timeoutMs,signal:controller.signal});
    await fs.writeFile(path.join(state,'host.stdout.log'),host.stdout);await fs.writeFile(path.join(state,'host.stderr.log'),host.stderr);report.hostExitCode=host.exitCode;
    if(host.exitCode!==0) throw issue('HOST_EXIT',`Extension Host exited ${host.exitCode}; inspect host logs`);
    const evidence=await json(hostReportPath);await verifyHostReport(evidence,{extensionRoot,extensionId:id,version:manifest.version,engineSha256:report.engineSha256,vscodeVersion:vscode.version});
    if(await sha(engine)!==report.engineSha256||await sha(vsix)!==report.vsixSha256) throw issue('INPUT_CHANGED','Release artifact changed during installed verification');
    report.results=evidence.results;report.host=evidence;report.verified=true;
  } catch(error) {
    report.error={code:error.code??'HOST_HARNESS',message:String(error.message??error)};
    if(typeof error.stdout==='string') await fs.writeFile(path.join(state,'failure.stdout.log'),error.stdout);
    if(typeof error.stderr==='string') await fs.writeFile(path.join(state,'failure.stderr.log'),error.stderr);
  }
  finally {
    process.removeListener('SIGINT',cancel);process.removeListener('SIGTERM',cancel);
    if(profile?.temporary) {
      // runProcess has observed the editor exit (or terminated its process
      // group) before this point. Keep failure diagnostics in durable evidence.
      if(!report.verified) {
        try {await fs.cp(path.join(profile.directory,'logs'),path.join(state,'user-data-logs'),{recursive:true,dereference:false,verbatimSymlinks:true});}
        catch(error) {if(error.code!=='ENOENT') report.profileLogError={code:error.code,message:error.message};}
      }
      try {await profile.cleanup();report.userDataRemoved=true;}
      catch(error) {report.verified=false;report.cleanupError={code:error.code??'PROFILE_CLEANUP',message:error.message};report.error??=report.cleanupError;}
    }
    report.completedAt=new Date().toISOString();
    const serialized=JSON.stringify(report,null,2)+'\n';await fs.writeFile(path.join(state,'summary.json'),serialized,{flag:'wx'});
    await fs.writeFile(path.join(state,'summary.sha256'),createHash('sha256').update(serialized).digest('hex')+'  summary.json\n',{flag:'wx'});
    await fs.writeFile(path.join(output,'latest.json'),JSON.stringify({report:path.relative(output,path.join(state,'summary.json'))},null,2)+'\n');
  }
  console.log(JSON.stringify({verified:report.verified,report:path.join(state,'summary.json'),checks:report.results?.length??0,error:report.error},null,2));
  return report.verified?0:1;
}

if(process.argv[1]&&path.resolve(process.argv[1])===filename) {
  if(process.argv[2]==='--internal-download') {
    const options=await json(process.argv[3]);
    const {downloadAndUnzipVSCode}=await import('@vscode/test-electron');
    const executable=await downloadAndUnzipVSCode({version:options.version,cachePath:options.cachePath,timeout:options.timeout});
    await fs.writeFile(options.result,JSON.stringify({executable}),{flag:'wx'});
  } else main().then(code=>{process.exitCode=code;},error=>{console.error(`${error.code??'HOST_HARNESS'}: ${error.message}`);process.exitCode=2;});
}
