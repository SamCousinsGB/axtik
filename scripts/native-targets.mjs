import fs from 'node:fs';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';

export const nativeTargets = Object.freeze(['win32-x64','win32-arm64','darwin-x64','darwin-arm64','linux-x64','linux-arm64']);
export const executableName = target => target.startsWith('win32-') ? 'apx.exe' : 'apx';
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const fail = message => {throw new Error(message);};
const range = (bytes,start,length) => Number.isSafeInteger(start) && Number.isSafeInteger(length) && start>=0 && length>=0 && start<=bytes.length && length<=bytes.length-start;
const offset = value => value<=BigInt(Number.MAX_SAFE_INTEGER) ? Number(value) : fail('Native binary offset exceeds supported bounds.');
const windowsSystemLibraries=new Set(['advapi32.dll','bcrypt.dll','bcryptprimitives.dll','crypt32.dll','cryptbase.dll','dbghelp.dll','gdi32.dll','imm32.dll','iphlpapi.dll','kernel32.dll','kernelbase.dll','msvcrt.dll','mswsock.dll','ncrypt.dll','netapi32.dll','normaliz.dll','ntdll.dll','ole32.dll','oleaut32.dll','powrprof.dll','psapi.dll','rpcrt4.dll','secur32.dll','setupapi.dll','shell32.dll','shlwapi.dll','sspicli.dll','ucrtbase.dll','user32.dll','userenv.dll','version.dll','winhttp.dll','wininet.dll','winmm.dll','wintrust.dll','wldap32.dll','ws2_32.dll','wsock32.dll','wtsapi32.dll']);
const isWindowsSystemLibrary=name=>windowsSystemLibraries.has(name)||/^(?:api|ext)-ms-win-[a-z0-9-]+\.dll$/.test(name);

// PE32+ import/delay-import layouts follow Microsoft's PE/COFF specification.
function peDependencies(bytes,pe) {
  const optional=pe+24,optionalSize=bytes.readUInt16LE(pe+20),sectionCount=bytes.readUInt16LE(pe+6);
  if(optionalSize<112 || !range(bytes,optional,optionalSize) || !sectionCount || sectionCount>96 || !range(bytes,optional+optionalSize,sectionCount*40)) fail('Malformed PE optional header or section table.');
  const directoryCount=bytes.readUInt32LE(optional+108),headerSize=bytes.readUInt32LE(optional+60);
  if(directoryCount>16 || optionalSize<112+directoryCount*8 || !range(bytes,0,headerSize)) fail('Malformed PE data directories.');
  const sections=[];
  for(let i=0;i<sectionCount;i++) {
    const base=optional+optionalSize+i*40;
    const rva=bytes.readUInt32LE(base+12),size=bytes.readUInt32LE(base+16),start=bytes.readUInt32LE(base+20);
    if(!range(bytes,start,size)) fail('Malformed PE section bounds.');
    sections.push({rva,size,start});
  }
  function map(rva,length) {
    if(rva<headerSize && range(bytes,rva,length) && rva+length<=headerSize) return rva;
    const matches=sections.filter(s=>rva>=s.rva && rva-s.rva<=s.size-length);
    if(matches.length!==1) fail('PE import RVA is outside one unambiguous file-backed section.');
    return matches[0].start+rva-matches[0].rva;
  }
  function nameAt(rva) {
    const characters=[];
    for(let i=0;i<256;i++) {
      const byte=bytes[map(rva+i,1)];
      if(byte===0) {if(!characters.length) fail('Empty PE import name.');return String.fromCharCode(...characters).toLowerCase();}
      if(byte<32 || byte>126) fail('Invalid PE import name.');characters.push(byte);
    }
    fail('Unterminated PE import name.');
  }
  const names=[];
  for(const [index,size,nameOffset] of [[1,20,12],[13,32,4]]) {
    if(directoryCount<=index) continue;
    const descriptor=optional+112+index*8,rva=bytes.readUInt32LE(descriptor),length=bytes.readUInt32LE(descriptor+4);
    if(!rva && !length) continue;
    if(!rva || length<size || length>1024*size) fail('Invalid PE import directory size.');
    let terminated=false;
    for(let i=0;i+size<=length;i+=size) {
      const at=map(rva+i,size),entry=bytes.subarray(at,at+size);
      if(entry.every(byte=>byte===0)) {terminated=true;break;}
      if(index===13 && bytes.readUInt32LE(at)!==1) fail('Unsupported PE delayed-import addressing.');
      names.push(nameAt(bytes.readUInt32LE(at+nameOffset)));
    }
    if(!terminated) fail('Unterminated PE import directory.');
  }
  return [...new Set(names)].sort();
}

/** Inspect the executable itself instead of trusting its filename or CI label. */
export function inspectNative(bytes) {
  if(!Buffer.isBuffer(bytes)) fail('Native input must be a Buffer.');
  if(bytes.length>=64 && bytes.toString('ascii',0,2)==='MZ') {
    const pe=bytes.readUInt32LE(0x3c);
    if(!range(bytes,pe,26)||bytes.readUInt32LE(pe)!==0x4550) fail('Malformed PE executable.');
    const machine=bytes.readUInt16LE(pe+4), flags=bytes.readUInt16LE(pe+22);
    const arch=machine===0x8664?'x64':machine===0xaa64?'arm64':undefined;
    if(!arch || !(flags&2) || (flags&0x2000) || bytes.readUInt16LE(pe+24)!==0x20b) fail('Expected a Windows x64 or ARM64 executable, not a DLL.');
    return {target:`win32-${arch}`,format:'PE32+',linkage:'windows',dependencies:peDependencies(bytes,pe)};
  }
  if(bytes.length>=64 && bytes.subarray(0,4).equals(Buffer.from([0x7f,0x45,0x4c,0x46]))) {
    if(bytes[4]!==2 || bytes[5]!==1 || bytes[6]!==1 || ![2,3].includes(bytes.readUInt16LE(16))) fail('Expected a little-endian ELF64 executable.');
    const machine=bytes.readUInt16LE(18), arch=machine===62?'x64':machine===183?'arm64':undefined;
    if(!arch) fail('Unsupported ELF architecture.');
    const table=offset(bytes.readBigUInt64LE(32)), size=bytes.readUInt16LE(54), count=bytes.readUInt16LE(56);
    if(size<56 || !count || count===65535 || !range(bytes,table,size*count)) fail('Malformed ELF program-header table.');
    let interpreter=false,needed=false;
    for(let i=0;i<count;i++) {
      const base=table+i*size, type=bytes.readUInt32LE(base);
      const start=offset(bytes.readBigUInt64LE(base+8)), length=offset(bytes.readBigUInt64LE(base+32));
      if(!range(bytes,start,length)) fail('Malformed ELF segment bounds.');
      if(type===3) interpreter=true;
      if(type===2) {
        if(length%16) fail('Malformed ELF dynamic segment.');
        let terminated=false;
        for(let entry=start;entry<start+length;entry+=16) {
          const tag=bytes.readBigInt64LE(entry);
          if(tag===0n) {terminated=true;break;}
          if(tag===1n) needed=true;
        }
        if(!terminated) fail('Unterminated ELF dynamic segment.');
      }
    }
    return {target:`linux-${arch}`,format:'ELF64',linkage:interpreter||needed?'dynamic':'static',interpreter,needed};
  }
  if(bytes.length>=32 && bytes.readUInt32LE(0)===0xfeedfacf) {
    const cpu=bytes.readUInt32LE(4), arch=cpu===0x01000007?'x64':cpu===0x0100000c?'arm64':undefined;
    if(!arch || bytes.readUInt32LE(12)!==2) fail('Expected a macOS x64 or ARM64 Mach-O executable.');
    const count=bytes.readUInt32LE(16), commandBytes=bytes.readUInt32LE(20);
    if(!range(bytes,32,commandBytes) || count>commandBytes/8) fail('Malformed Mach-O load commands.');
    let at=32;const dependencies=[];
    for(let i=0;i<count;i++) {
      if(!range(bytes,at,8)) fail('Malformed Mach-O command header.');
      const size=bytes.readUInt32LE(at+4);
      if(size<8 || size%8 || !range(bytes,at,size) || at+size>32+commandBytes) fail('Malformed Mach-O load command.');
      const command=bytes.readUInt32LE(at);
      if([0xc,0x80000018,0x8000001f,0x20,0x80000023].includes(command)) {
        if(size<24) fail('Malformed Mach-O dylib command.');
        const nameOffset=bytes.readUInt32LE(at+8);
        if(nameOffset<24 || nameOffset>=size) fail('Malformed Mach-O dylib name offset.');
        const end=bytes.indexOf(0,at+nameOffset);
        if(end<0 || end>=at+size) fail('Unterminated Mach-O dylib name.');
        dependencies.push(bytes.toString('utf8',at+nameOffset,end));
      }
      at+=size;
    }
    if(at!==32+commandBytes) fail('Mach-O load command accounting differs.');
    return {target:`darwin-${arch}`,format:'Mach-O64',linkage:'macos',dependencies:[...new Set(dependencies)].sort()};
  }
  fail('Unrecognized native executable format.');
}

export function verifyNative(bytes,target,{requireStaticLinux=false}={}) {
  if(!nativeTargets.includes(target)) fail(`Unsupported native target ${target}.`);
  const info=inspectNative(bytes);
  if(info.target!==target) fail(`Executable is ${info.target}; refusing to label it ${target}.`);
  if(target.startsWith('win32-') && info.dependencies.some(name=>!isWindowsSystemLibrary(name))) fail(`Windows engine requires a non-OS runtime DLL: ${info.dependencies.filter(name=>!isWindowsSystemLibrary(name)).join(', ')}. Build with a static CRT.`);
  if(target.startsWith('darwin-') && info.dependencies.some(name=>path.posix.normalize(name)!==name||!name.startsWith('/usr/lib/')&&!name.startsWith('/System/Library/Frameworks/'))) fail('macOS engine depends on a non-system dylib.');
  if(requireStaticLinux && target.startsWith('linux-') && info.linkage!=='static') fail('Universal Linux engines must be statically linked, without PT_INTERP or DT_NEEDED. Build the musl target.');
  return info;
}

export function observeNative(engine,target,version,{requireStaticLinux=true}={}) {
  const bytes=fs.readFileSync(engine), info=verifyNative(bytes,target,{requireStaticLinux});
  if(target!==`${process.platform}-${process.arch}`) fail(`A ${target} build receipt must be observed on a matching native host; this host is ${process.platform}-${process.arch}.`);
  const result=spawnSync(engine,['--version'],{shell:false,encoding:'utf8',timeout:10000,maxBuffer:65536,windowsHide:true});
  const expected=`apx ${version}`;
  if(result.error || result.status!==0 || result.stdout.trim()!==expected) fail(`Native executable did not report ${expected} on ${target}.`);
  if(sha256(fs.readFileSync(engine))!==sha256(bytes)) fail('Native executable changed during version verification.');
  return {schemaVersion:1,target,version,sha256:sha256(bytes),bytes:bytes.length,observedVersion:expected,host:{platform:process.platform,arch:process.arch},format:info.format,linkage:info.linkage,dependencies:info.dependencies??[]};
}

export function verifyReceipt(receipt,bytes,target,version,{requireStaticLinux=false}={}) {
  const info=verifyNative(bytes,target,{requireStaticLinux});
  const [platform,arch]=target.split('-');
  if(receipt?.schemaVersion!==1 || receipt.target!==target || receipt.version!==version || receipt.observedVersion!==`apx ${version}` || receipt.sha256!==sha256(bytes) || receipt.bytes!==bytes.length || receipt.host?.platform!==platform || receipt.host?.arch!==arch || receipt.format!==info.format || receipt.linkage!==info.linkage || JSON.stringify(receipt.dependencies)!==JSON.stringify(info.dependencies??[])) fail(`Native build receipt does not match the ${target} executable/version/host.`);
  return receipt;
}

export function loadEngineBundle(directory,version) {
  return nativeTargets.map(target=>{
    const engine=path.resolve(directory,target,executableName(target));
    if(!fs.existsSync(engine)) fail(`Universal bundle is missing ${target}/${executableName(target)}.`);
    const bytes=fs.readFileSync(engine), receiptPath=path.join(path.dirname(engine),'engine.json');
    if(!fs.existsSync(receiptPath)) fail(`Universal bundle is missing ${target}/engine.json.`);
    const receipt=verifyReceipt(JSON.parse(fs.readFileSync(receiptPath,'utf8')),bytes,target,version,{requireStaticLinux:true});
    return {target,engine,receipt};
  });
}

function main(args) {
  if(args[0]!=='verify') fail('Usage: native-targets.mjs verify --target TARGET --engine FILE --version VERSION --output engine.json');
  const options={};
  for(let i=1;i<args.length;i+=2) {
    if(!['--target','--engine','--version','--output'].includes(args[i]) || !args[i+1] || options[args[i]]) fail(`Unknown, duplicate or incomplete option: ${args[i]}`);
    options[args[i]]=args[i+1];
  }
  if(Object.keys(options).length!==4) fail('All --target, --engine, --version and --output options are required.');
  const receipt=observeNative(path.resolve(options['--engine']),options['--target'],options['--version']);
  fs.mkdirSync(path.dirname(path.resolve(options['--output'])),{recursive:true});
  fs.writeFileSync(options['--output'],JSON.stringify(receipt,null,2)+'\n');
  console.log(JSON.stringify(receipt));
}
if(process.argv[1] && path.resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  try {main(process.argv.slice(2));} catch(error) {console.error(error.message);process.exitCode=1;}
}
