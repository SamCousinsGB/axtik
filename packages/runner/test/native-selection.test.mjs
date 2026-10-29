import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {mkdtemp,mkdir,writeFile,readFile,chmod,stat,rm,realpath,symlink} from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {bundledEngineTarget,resolveBundledEngine} from '../lib/index.js';

async function fixture(t,{mode=0o755,receipt=true}={}) {
  const parent=await realpath(os.tmpdir()), root=await mkdtemp(path.join(parent,'axtik-native-resolution-'));
  t.after(async()=>{assert.equal(path.dirname(await realpath(root)),parent);await rm(root,{recursive:true,force:true});});
  const target=bundledEngineTarget(), folder=path.join(root,'bin',target), engine=path.join(folder,process.platform==='win32'?'apx.exe':'apx');
  await mkdir(folder,{recursive:true});await writeFile(engine,'test fixture only');await chmod(engine,mode);
  await writeFile(path.join(root,'package.json'),JSON.stringify({version:'fixture'}));
  if(receipt) await writeFile(path.join(folder,'engine.json'),JSON.stringify({schemaVersion:1,target,version:'fixture',sha256:createHash('sha256').update(await readFile(engine)).digest('hex'),bytes:(await stat(engine)).size}));
  return {root,engine,folder};
}

test('selects the extension host OS and architecture for all six native bundles',()=>{
  for(const platform of ['win32','darwin','linux']) for(const arch of ['x64','arm64']) assert.equal(bundledEngineTarget(platform,arch),`${platform}-${arch}`);
  for(const [platform,arch] of [['win32','ia32'],['linux','arm'],['freebsd','x64'],['web','x64']]) assert.throws(()=>bundledEngineTarget(platform,arch),e=>e.code==='APX_PLATFORM_UNSUPPORTED');
});
test('chooses the local bundled engine without an installed runtime or explicit override',async t=>{
  const {root,engine}=await fixture(t);assert.equal(await resolveBundledEngine(root),engine);
  assert.equal(await resolveBundledEngine(root,process.execPath),process.execPath);
});
test('never uses a linked bundle directory outside the installed package',async t=>{
  const {root,engine}=await fixture(t);const outside=await mkdtemp(path.join(await realpath(os.tmpdir()),'axtik-native-outside-'));
  t.after(async()=>{assert.ok(path.basename(outside).startsWith('axtik-native-outside-'));await rm(outside,{recursive:true,force:true});});
  await rm(path.join(root,'bin'),{recursive:true});
  await symlink(outside,path.join(root,'bin'),process.platform==='win32'?'junction':'dir');
  await assert.rejects(resolveBundledEngine(root),e=>e.code==='APX_ENGINE_MISSING');
});
test('repairs a missing Unix owner execute bit only after verifying the bundled digest',{skip:process.platform==='win32'},async t=>{
  const {root,engine}=await fixture(t,{mode:0o600});
  assert.equal(await resolveBundledEngine(root),engine);assert.equal((await stat(engine)).mode&0o777,0o700);
});
test('does not change Unix permissions for altered bundles or explicit overrides',{skip:process.platform==='win32'},async t=>{
  const {root,engine}=await fixture(t,{mode:0o600});
  await assert.rejects(resolveBundledEngine(root,engine),e=>e.code==='APX_ENGINE_MISSING');
  assert.equal((await stat(engine)).mode&0o777,0o600);
  await writeFile(engine,'changed fixture!!');
  await assert.rejects(resolveBundledEngine(root),e=>e.code==='APX_ENGINE_PERMISSIONS');
  assert.equal((await stat(engine)).mode&0o777,0o600);
});
test('unverified legacy bundles are never granted execute permission automatically',{skip:process.platform==='win32'},async t=>{
  const {root,engine}=await fixture(t,{mode:0o600,receipt:false});
  await assert.rejects(resolveBundledEngine(root),e=>e.code==='APX_ENGINE_PERMISSIONS');
  assert.equal((await stat(engine)).mode&0o777,0o600);
});
