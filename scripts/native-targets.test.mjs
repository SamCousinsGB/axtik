import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {nativeTargets,executableName,sha256,inspectNative,verifyNative,verifyReceipt,loadEngineBundle,observeNative} from './native-targets.mjs';

// Synthetic executable headers test rejection logic; they are never run and
// never represented as verified builds in release evidence.
function header(target,{interpreter=false,needed=false}={}) {
  if(target.startsWith('win32-')) {
    const bytes=Buffer.alloc(1024);bytes.write('MZ');bytes.writeUInt32LE(64,0x3c);bytes.writeUInt32LE(0x4550,64);
    bytes.writeUInt16LE(target.endsWith('x64')?0x8664:0xaa64,68);bytes.writeUInt16LE(1,70);bytes.writeUInt16LE(240,84);bytes.writeUInt16LE(2,86);bytes.writeUInt16LE(0x20b,88);
    bytes.writeUInt32LE(512,148);bytes.writeUInt32LE(16,196);bytes.writeUInt32LE(4096,340);bytes.writeUInt32LE(512,344);bytes.writeUInt32LE(512,348);return bytes;
  }
  if(target.startsWith('darwin-')) {
    const bytes=Buffer.alloc(32);bytes.writeUInt32LE(0xfeedfacf);bytes.writeUInt32LE(target.endsWith('x64')?0x01000007:0x0100000c,4);bytes.writeUInt32LE(2,12);return bytes;
  }
  const bytes=Buffer.alloc(208);Buffer.from([0x7f,0x45,0x4c,0x46,2,1,1]).copy(bytes);bytes.writeUInt16LE(2,16);bytes.writeUInt16LE(target.endsWith('x64')?62:183,18);
  bytes.writeBigUInt64LE(64n,32);bytes.writeUInt16LE(56,54);bytes.writeUInt16LE(1,56);
  bytes.writeUInt32LE(interpreter?3:needed?2:1,64);bytes.writeBigUInt64LE(176n,72);bytes.writeBigUInt64LE(32n,96);
  if(needed) bytes.writeBigInt64LE(1n,176);
  return bytes;
}
function receipt(bytes,target,version='fixture') {
  const info=inspectNative(bytes);const [platform,arch]=target.split('-');
  return {schemaVersion:1,target,version,sha256:sha256(bytes),bytes:bytes.length,observedVersion:`apx ${version}`,host:{platform,arch},format:info.format,linkage:info.linkage,dependencies:info.dependencies??[]};
}
function bundle(t) {
  const parent=fs.realpathSync(os.tmpdir()), directory=fs.mkdtempSync(path.join(parent,'axtik-native-headers-'));
  t.after(()=>{assert.equal(path.dirname(fs.realpathSync(directory)),parent);fs.rmSync(directory,{recursive:true,force:true});});
  for(const target of nativeTargets) {
    const folder=path.join(directory,target);fs.mkdirSync(folder);const bytes=header(target);
    fs.writeFileSync(path.join(folder,executableName(target)),bytes);fs.writeFileSync(path.join(folder,'engine.json'),JSON.stringify(receipt(bytes,target)));
  }
  return directory;
}

test('six supported architecture headers resolve without trusting the filename',()=>{
  for(const target of nativeTargets) {
    assert.equal(verifyNative(header(target),target,{requireStaticLinux:true}).target,target);
    const wrong=target.endsWith('x64')?target.replace('x64','arm64'):target.replace('arm64','x64');
    assert.throws(()=>verifyNative(header(target),wrong),/refusing to label/);
  }
});
test('universal Linux binaries cannot depend on a dynamic loader or shared libc',()=>{
  for(const target of ['linux-x64','linux-arm64']) for(const options of [{interpreter:true},{needed:true}]) {
    const bytes=header(target,options);
    assert.equal(verifyNative(bytes,target).linkage,'dynamic');
    assert.throws(()=>verifyNative(bytes,target,{requireStaticLinux:true}),/statically linked/);
  }
});
test('Windows dependency checks reject runtime redistributables including delayed imports',()=>{
  for(const delayed of [false,true]) for(const dll of ['vcruntime140.dll','msvcp140.dll','libgcc_s_seh-1.dll','libstdc++-6.dll','libwinpthread-1.dll']) {
    const bytes=header('win32-x64');const directory=88+112+(delayed?13:1)*8;
    bytes.writeUInt32LE(4096,directory);bytes.writeUInt32LE(delayed?64:40,directory+4);
    if(delayed) bytes.writeUInt32LE(1,512);
    bytes.writeUInt32LE(4096+288,512+(delayed?4:12));bytes.write(dll+'\0',800,'ascii');
    assert.deepEqual(inspectNative(bytes).dependencies,[dll]);
    assert.throws(()=>verifyNative(bytes,'win32-x64'),/non-OS runtime DLL/);
  }
  const bytes=header('win32-arm64');bytes.writeUInt32LE(4096,208);bytes.writeUInt32LE(40,212);bytes.writeUInt32LE(4384,524);bytes.write('KERNEL32.dll\0',800,'ascii');
  assert.deepEqual(verifyNative(bytes,'win32-arm64').dependencies,['kernel32.dll']);
  bytes.writeUInt32LE(0x7fffffff,524);assert.throws(()=>inspectNative(bytes),/RVA/);
});
test('macOS dependencies must use system library paths rather than local package managers',()=>{
  for(const name of ['/opt/homebrew/lib/libextra.dylib','@rpath/libextra.dylib','/usr/lib/../../tmp/extra.dylib','/usr/lib/libSystem.B.dylib']) {
    const size=Math.ceil((24+Buffer.byteLength(name)+1)/8)*8, bytes=Buffer.alloc(32+size);header('darwin-arm64').copy(bytes);
    bytes.writeUInt32LE(1,16);bytes.writeUInt32LE(size,20);bytes.writeUInt32LE(0xc,32);bytes.writeUInt32LE(size,36);bytes.writeUInt32LE(24,40);bytes.write(name+'\0',56);
    if(name==='/usr/lib/libSystem.B.dylib') assert.deepEqual(verifyNative(bytes,'darwin-arm64').dependencies,[name]);
    else assert.throws(()=>verifyNative(bytes,'darwin-arm64'),/non-system dylib/);
  }
});
test('malformed binary offsets, non-executables and truncated headers are rejected',()=>{
  for(const target of nativeTargets) {
    assert.throws(()=>inspectNative(header(target).subarray(0,20)));
  }
  const pe=header('win32-x64');pe.writeUInt16LE(0x2002,86);assert.throws(()=>inspectNative(pe),/not a DLL/);
  const elf=header('linux-x64');elf.writeBigUInt64LE(0xffffffffffffffffn,32);assert.throws(()=>inspectNative(elf),/offset/);
  const shared=header('linux-arm64',{needed:true});shared.fill(1,192);assert.throws(()=>inspectNative(shared),/Unterminated/);
  const macho=header('darwin-arm64');macho.writeUInt32LE(3,16);assert.throws(()=>inspectNative(macho),/load commands/);
});
test('build receipts bind version, actual host architecture, binary bytes and linkage',()=>{
  const bytes=header('linux-arm64'), proof=receipt(bytes,'linux-arm64');
  assert.equal(verifyReceipt(proof,bytes,'linux-arm64','fixture'),proof);
  for(const change of [{version:'older'},{observedVersion:'apx older'},{sha256:'0'.repeat(64)},{bytes:bytes.length+1},{host:{platform:'linux',arch:'x64'}},{linkage:'dynamic'}]) {
    assert.throws(()=>verifyReceipt({...proof,...change},bytes,'linux-arm64','fixture'),/does not match/);
  }
});
test('a universal bundle requires all six matching native receipts',t=>{
  const directory=bundle(t);assert.equal(loadEngineBundle(directory,'fixture').length,6);
  const missing=path.join(directory,'win32-arm64','engine.json');fs.rmSync(missing);
  assert.throws(()=>loadEngineBundle(directory,'fixture'),/missing win32-arm64\/engine.json/);
});
test('a universal bundle rejects swapped architecture payloads and changed bytes',t=>{
  const directory=bundle(t), file=path.join(directory,'darwin-x64','apx');
  fs.writeFileSync(file,header('darwin-arm64'));assert.throws(()=>loadEngineBundle(directory,'fixture'),/refusing to label/);
  const changed=header('darwin-x64');changed[31]=1;fs.writeFileSync(file,changed);
  assert.throws(()=>loadEngineBundle(directory,'fixture'),/does not match/);
});
test('native build evidence requires an actual matching version response',()=>{
  assert.throws(()=>observeNative(process.execPath,`${process.platform}-${process.arch}`,'fixture',{requireStaticLinux:false}),/did not report apx fixture/);
});
