import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {HOST_CHECKS,parseOptions,childEnvironment,runProcess,locateVSCode,findInstalledExtension,verifyHostReport} from './test-installed-vsix.mjs';

async function temporary(run) {
  const parent=await fs.realpath(os.tmpdir()),folder=await fs.mkdtemp(path.join(parent,'apx-installed-test-'));
  try {return await run(folder);} finally {assert.equal(path.dirname(folder),parent);await fs.rm(folder,{recursive:true,force:true});}
}
async function write(file,contents) {await fs.mkdir(path.dirname(file),{recursive:true});await fs.writeFile(file,contents);}
async function app(folder,platform,hashed=false) {
  const exe=path.join(folder,...(platform==='darwin'?['Code.app','Contents','MacOS','Electron']:[platform==='win32'?'Code.exe':'code']));
  const cli=path.join(folder,...(platform==='darwin'?['Code.app','Contents','Resources','app','out','cli.js']:[...(hashed?['0123456789']:[]),'resources','app','out','cli.js']));
  await write(exe,'fixture');await write(cli,'fixture');await write(path.join(path.dirname(cli),'..','package.json'),JSON.stringify({version:'1.144.0'}));
  if(hashed) await write(path.join(folder,'bin','code.cmd'),'@echo off\n"%~dp0..\\Code.exe" "%~dp0..\\0123456789\\resources\\app\\out\\cli.js" %*\n');
  return {exe,cli};
}

test('installed test options require actual artifacts and enforce bounded unique arguments',()=>{
  assert.equal(parseOptions(['--help']).help,true);
  const valid=parseOptions(['--vsix','release.vsix','--engine','apx','--vscode-version','1.144.0']);assert.equal(valid.vscodeVersion,'1.144.0');
  for(const args of [[],['--vsix','a'],['--vsix','a','--engine','b','--engine','c'],['--vsix','a','--engine','b','--timeout-ms','NaN'],['--vsix','a','--engine','b','--download-timeout-ms','1200001'],['--vsix','a','--engine','b','--vscode-version','https://untrusted'],['--vsix','a','--engine','b','--unknown','x']]) assert.throws(()=>parseOptions(args),{code:'ARGUMENT'});
});

test('locates Windows, hashed Windows, Linux and macOS CLI without shell launchers',async()=>{
  await temporary(async folder=>{
    for(const [platform,hashed] of [['win32',false],['win32',true],['linux',false],['darwin',false]]) {
      const base=path.join(folder,`${platform}-${hashed}`),fixture=await app(base,platform,hashed);
      const observed=await locateVSCode(fixture.exe,platform);
      assert.equal(observed.executable,await fs.realpath(fixture.exe));assert.equal(observed.cli,await fs.realpath(fixture.cli));assert.equal(observed.version,'1.144.0');
      const directory=platform==='darwin'?path.join(base,'Code.app'):base;
      assert.deepEqual(await locateVSCode(directory,platform),observed);
    }
  });
});

test('missing layouts and launcher traversal fail before launching any process',async()=>{
  await temporary(async folder=>{
    const exe=path.join(folder,'Code.exe');await write(exe,'fixture');
    await assert.rejects(locateVSCode(exe,'win32'),{code:'VSCODE_LAYOUT'});
    await write(path.join(folder,'bin','code.cmd'),'"%~dp0..\\Code.exe" "%~dp0..\\..\\outside\\resources\\app\\out\\cli.js"');
    await assert.rejects(locateVSCode(exe,'win32'),{code:'VSCODE_LAYOUT'});
  });
});

test('installed lookup requires one exact package identity and version',async()=>{
  await temporary(async folder=>{
    await write(path.join(folder,'unrelated','package.json'),JSON.stringify({publisher:'unrelated',name:'extension',version:'0.2.2'}));
    await write(path.join(folder,'apx-old','package.json'),JSON.stringify({publisher:'apx-local',name:'apx-local-apex',version:'0.2.1'}));
    await assert.rejects(findInstalledExtension(folder,'apx-local.apx-local-apex','0.2.2'),{code:'EXTENSION_INSTALL'});
    const good=path.join(folder,'arbitrary-folder-name');await write(path.join(good,'package.json'),JSON.stringify({publisher:'apx-local',name:'apx-local-apex',version:'0.2.2'}));
    assert.equal(await findInstalledExtension(folder,'apx-local.apx-local-apex','0.2.2'),await fs.realpath(good));
    await write(path.join(folder,'duplicate','package.json'),JSON.stringify({publisher:'apx-local',name:'apx-local-apex',version:'0.2.2'}));
    await assert.rejects(findInstalledExtension(folder,'apx-local.apx-local-apex','0.2.2'),{code:'EXTENSION_INSTALL'});
  });
});

test('host evidence must conserve every check and exact installed runtime identity',async()=>{
  await temporary(async folder=>{
    const expected={extensionRoot:folder,extensionId:'apx-local.apx-local-apex',version:'0.2.2',engineSha256:'a'.repeat(64),vscodeVersion:'1.144.0',platform:'linux',arch:'arm64'};
    const good={schemaVersion:1,...expected,results:HOST_CHECKS.map(name=>({name,passed:true}))};
    await verifyHostReport(good,expected);
    for(const mutate of [r=>r.results.pop(),r=>r.results[1]={...r.results[0]},r=>r.results[0].passed=false,r=>r.results[0].name='unknown',r=>r.engineSha256='b'.repeat(64),r=>r.arch='x64',r=>r.platform='darwin',r=>r.version='0.2.1',r=>r.vscodeVersion='1.90.0',r=>r.extensionId='different.extension',r=>r.schemaVersion=0,r=>r.extensionRoot=path.dirname(folder)]) {
      const value=structuredClone(good);mutate(value);await assert.rejects(verifyHostReport(value,expected),{code:'HOST_EVIDENCE'});
    }
  });
});

test('test subprocess environment excludes auth and accidental Electron node mode',()=>{
  const prior=process.env.GITHUB_TOKEN,priorMode=process.env.ELECTRON_RUN_AS_NODE;
  try {process.env.GITHUB_TOKEN='fixture-secret';process.env.ELECTRON_RUN_AS_NODE='1';const env=childEnvironment({APX_HOST_TEST_REPORT:'fixture'});assert.equal(env.GITHUB_TOKEN,undefined);assert.equal(env.ELECTRON_RUN_AS_NODE,undefined);assert.equal(env.APX_HOST_TEST_REPORT,'fixture');assert.equal(childEnvironment({ELECTRON_RUN_AS_NODE:'1'}).ELECTRON_RUN_AS_NODE,'1');}
  finally {if(prior===undefined) delete process.env.GITHUB_TOKEN;else process.env.GITHUB_TOKEN=prior;if(priorMode===undefined) delete process.env.ELECTRON_RUN_AS_NODE;else process.env.ELECTRON_RUN_AS_NODE=priorMode;}
});

test('process runner preserves literal argv and distinguishes real exits',async()=>{
  const literal='a space & $(not-a-command) ; "quoted"';
  const result=await runProcess(process.execPath,['-e','process.stdout.write(process.argv[1]);process.stderr.write("fixture");process.exitCode=7',literal]);
  assert.equal(result.exitCode,7);assert.equal(result.stdout,literal);assert.equal(result.stderr,'fixture');
  await assert.rejects(runProcess(path.join(os.tmpdir(),'apx-definitely-missing-executable'),[]),{code:'PROCESS_START'});
});

test('process runner cancels and bounds hanging or noisy children',{timeout:30000},async()=>{
  // Cold CI hosts need time to start Node before the deliberately hanging child
  // emits the output whose preservation this timeout case verifies.
  const started=performance.now();
  await assert.rejects(runProcess(process.execPath,['-e','process.stdout.write("started");process.stderr.write("diagnostic");setInterval(()=>{},1000)'],{timeoutMs:5000}),error=>{assert.equal(error.code,'TIMEOUT');assert.equal(error.stdout,'started');assert.equal(error.stderr,'diagnostic');return true;});
  assert.ok(performance.now()-started<15000,'Timed-out child must terminate within the deadline plus process-tree cleanup allowance');
  await assert.rejects(runProcess(process.execPath,['-e','process.stdout.write("x".repeat(200000));setInterval(()=>{},1000)'],{maxOutputBytes:1024}),{code:'OUTPUT_LIMIT'});
  await assert.rejects(runProcess(process.execPath,['-e','setInterval(()=>{},1000)'],{signal:AbortSignal.timeout(100)}),{code:'CANCELLED'});
  const controller=new AbortController();controller.abort();await assert.rejects(runProcess('never-start-this',[],{signal:controller.signal}),{code:'CANCELLED'});
});
