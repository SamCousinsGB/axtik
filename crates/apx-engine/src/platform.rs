//! Pure native platform operations. No source-specific dispatch, network calls or org state.
//! Crypto algorithms follow Salesforce's public Crypto documentation. All results
//! remain subject to the engine's experimental assurance policy.
use crate::value::{NumKind, Number, Value};
use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use num_traits::ToPrimitive;
use rand::{rngs::OsRng, RngCore};
use rsa::{
    pkcs8::{DecodePrivateKey, DecodePublicKey},
    traits::PublicKeyParts,
    Pkcs1v15Sign, RsaPrivateKey, RsaPublicKey,
};
use sha2::{Digest, Sha256, Sha384, Sha512};
use subtle::ConstantTimeEq;

const MAX_BYTES: usize = 1_048_576;

#[derive(Debug)]
pub struct PlatformError {
    pub exception_type: &'static str,
    pub message: String,
    pub unsupported: bool,
}
type P<T> = Result<T, PlatformError>;
impl PlatformError {
    pub(crate) fn exception(typ: &'static str, message: impl Into<String>) -> Self {
        Self {
            exception_type: typ,
            message: message.into(),
            unsupported: false,
        }
    }
    pub(crate) fn unsupported(message: impl Into<String>) -> Self {
        Self {
            exception_type: "UnsupportedOperation",
            message: message.into(),
            unsupported: true,
        }
    }
}
fn arity(args: &[Value], count: usize) -> P<()> {
    if args.len() == count {
        Ok(())
    } else {
        Err(PlatformError::unsupported(format!(
            "Platform signature expects {count} arguments, received {}",
            args.len()
        )))
    }
}
fn text(value: &Value) -> P<&str> {
    match value {
        Value::Str(s) => Ok(s),
        Value::Null => Err(PlatformError::exception(
            "NullPointerException",
            "Argument cannot be null",
        )),
        _ => Err(PlatformError::unsupported(
            "Platform signature requires String",
        )),
    }
}
fn bytes(value: &Value) -> P<&[u8]> {
    match value {
        Value::Blob(b) if b.len() <= MAX_BYTES + 32 => Ok(b),
        Value::Blob(_) => Err(PlatformError::unsupported(
            "Blob exceeds bounded host operation size",
        )),
        Value::Null => Err(PlatformError::exception(
            "NullPointerException",
            "Argument cannot be null",
        )),
        _ => Err(PlatformError::unsupported(
            "Platform signature requires Blob",
        )),
    }
}
fn integer(value: &Value) -> P<i64> {
    match value {
        Value::Num(n) if n.q.is_integer() => n.q.to_integer().to_i64().ok_or_else(|| {
            PlatformError::exception("MathException", "Integer argument out of range")
        }),
        Value::Null => Err(PlatformError::exception(
            "NullPointerException",
            "Argument cannot be null",
        )),
        _ => Err(PlatformError::unsupported(
            "Platform signature requires Integer",
        )),
    }
}
fn bounded(data: &[u8]) -> P<()> {
    if data.len() > MAX_BYTES {
        Err(PlatformError::unsupported(
            "Platform operation exceeds 1 MiB host data bound",
        ))
    } else {
        Ok(())
    }
}
fn random(size: usize) -> P<Vec<u8>> {
    let mut out = vec![0; size];
    OsRng
        .try_fill_bytes(&mut out)
        .map_err(|_| PlatformError::unsupported("Host random source unavailable"))?;
    Ok(out)
}
fn digest(algorithm: &str, data: &[u8]) -> P<Vec<u8>> {
    bounded(data)?;
    Ok(match algorithm.to_ascii_uppercase().as_str() {
        "MD5" => md5::Md5::digest(data).to_vec(),
        "SHA1" | "SHA-1" => sha1::Sha1::digest(data).to_vec(),
        "SHA256" | "SHA-256" => Sha256::digest(data).to_vec(),
        "SHA384" | "SHA-384" => Sha384::digest(data).to_vec(),
        "SHA512" | "SHA-512" => Sha512::digest(data).to_vec(),
        other => {
            return Err(PlatformError::unsupported(format!(
                "Digest algorithm {other} is not implemented"
            )))
        }
    })
}
fn mac(algorithm: &str, data: &[u8], key: &[u8]) -> P<Vec<u8>> {
    bounded(data)?;
    if key.len() > 4096 {
        return Err(PlatformError::exception(
            "InvalidParameterValue",
            "HMAC key exceeds 4096 bytes",
        ));
    }
    macro_rules! run {
        ($d:ty) => {{
            let mut h = <Hmac<$d> as Mac>::new_from_slice(key).map_err(|_| {
                PlatformError::exception("InvalidParameterValue", "Invalid HMAC key")
            })?;
            h.update(data);
            h.finalize().into_bytes().to_vec()
        }};
    }
    Ok(match algorithm.to_ascii_uppercase().as_str() {
        "HMACMD5" => run!(md5::Md5),
        "HMACSHA1" => run!(sha1::Sha1),
        "HMACSHA256" => run!(Sha256),
        "HMACSHA384" => run!(Sha384),
        "HMACSHA512" => run!(Sha512),
        other => {
            return Err(PlatformError::unsupported(format!(
                "HMAC algorithm {other} is not implemented"
            )))
        }
    })
}
fn aes(algorithm: &str, key: &[u8], iv: &[u8], data: &[u8], decrypt: bool) -> P<Vec<u8>> {
    if !["AES128", "AES192", "AES256"].contains(&algorithm.to_ascii_uppercase().as_str()) {
        return Err(PlatformError::unsupported(format!(
            "Cipher algorithm {algorithm} is not implemented"
        )));
    }
    if iv.len() != 16 {
        return Err(PlatformError::exception(
            "InvalidParameterValue",
            "Invalid initialization vector. Must be 16 bytes.",
        ));
    }
    if data.len() > MAX_BYTES + if decrypt { 32 } else { 0 } {
        return Err(PlatformError::exception(
            "InvalidParameterValue",
            "Invalid data. Input exceeds encryption size limit.",
        ));
    }
    // Salesforce permits the key's actual 128/192/256-bit size independently of the AES algorithm label.
    macro_rules! run {
        ($cipher:ty) => {{
            if decrypt {
                cbc::Decryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(|_| {
                        PlatformError::exception("InvalidParameterValue", "Invalid AES key")
                    })?
                    .decrypt_padded_vec_mut::<Pkcs7>(data)
                    .map_err(|_| {
                        PlatformError::exception(
                            "SecurityException",
                            "Given final block isn't properly padded.",
                        )
                    })
            } else {
                Ok(cbc::Encryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(|_| {
                        PlatformError::exception("InvalidParameterValue", "Invalid AES key")
                    })?
                    .encrypt_padded_vec_mut::<Pkcs7>(data))
            }
        }};
    }
    match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        _ => Err(PlatformError::exception(
            "InvalidParameterValue",
            "Invalid private key. Must be 16, 24, or 32 bytes.",
        )),
    }
}
fn signature_digest(algorithm: &str, data: &[u8]) -> P<(Pkcs1v15Sign, Vec<u8>)> {
    bounded(data)?;
    Ok(match algorithm.to_ascii_uppercase().as_str() {
        "RSA" | "RSA-SHA1" => (
            Pkcs1v15Sign::new::<sha1::Sha1>(),
            sha1::Sha1::digest(data).to_vec(),
        ),
        "RSA-SHA256" => (Pkcs1v15Sign::new::<Sha256>(), Sha256::digest(data).to_vec()),
        "RSA-SHA384" => (Pkcs1v15Sign::new::<Sha384>(), Sha384::digest(data).to_vec()),
        "RSA-SHA512" => (Pkcs1v15Sign::new::<Sha512>(), Sha512::digest(data).to_vec()),
        other => {
            return Err(PlatformError::unsupported(format!(
                "Signature algorithm {other} is not implemented"
            )))
        }
    })
}
fn sign(algorithm: &str, data: &[u8], key: &[u8]) -> P<Vec<u8>> {
    if key.len() > 4096 {
        return Err(PlatformError::exception(
            "InvalidParameterValue",
            "Private key exceeds 4096 bytes",
        ));
    }
    let (scheme, hash) = signature_digest(algorithm, data)?;
    let key = RsaPrivateKey::from_pkcs8_der(key).map_err(|_| {
        PlatformError::exception("CryptoException", "Invalid PKCS#8 RSA private key")
    })?;
    if key.n().bits() > 4096 {
        return Err(PlatformError::unsupported(
            "RSA modulus exceeds bounded 4096-bit host implementation",
        ));
    }
    key.sign_with_rng(&mut OsRng, scheme, &hash)
        .map_err(|_| PlatformError::exception("CryptoException", "RSA signing failed"))
}
fn verify(algorithm: &str, data: &[u8], signature: &[u8], key: &[u8]) -> P<bool> {
    if key.len() > 4096 {
        return Err(PlatformError::exception(
            "InvalidParameterValue",
            "Public key exceeds 4096 bytes",
        ));
    }
    let (scheme, hash) = signature_digest(algorithm, data)?;
    let key = RsaPublicKey::from_public_key_der(key)
        .map_err(|_| PlatformError::exception("CryptoException", "Invalid X.509 RSA public key"))?;
    if key.n().bits() > 4096 {
        return Err(PlatformError::unsupported(
            "RSA modulus exceeds bounded 4096-bit host implementation",
        ));
    }
    Ok(key.verify(scheme, &hash, signature).is_ok())
}

pub fn invoke(namespace: &str, method: &str, args: &[Value]) -> P<Option<Value>> {
    let namespace = namespace.to_ascii_lowercase();
    let namespace = namespace.strip_prefix("system.").unwrap_or(&namespace);
    let method = method.to_ascii_lowercase();
    let value = match (namespace, method.as_str()) {
        ("string", "escapesinglequotes") => {
            arity(args, 1)?;
            Value::Str(text(&args[0])?.replace('\'', "\\'"))
        }
        ("string", "join") => {
            arity(args, 2)?;
            let separator = text(&args[1])?;
            let values = match &args[0] {
                Value::List(v) | Value::Set(v) | Value::Iterable(v) => v.borrow(),
                Value::Null => {
                    return Err(PlatformError::exception(
                        "NullPointerException",
                        "Iterable cannot be null",
                    ))
                }
                _ => {
                    return Err(PlatformError::unsupported(
                        "String.join requires an Iterable",
                    ))
                }
            };
            let strings = values
                .iter()
                .map(|v| text(v).map(str::to_string))
                .collect::<P<Vec<_>>>()?;
            let size = strings
                .iter()
                .map(String::len)
                .sum::<usize>()
                .saturating_add(
                    separator
                        .len()
                        .saturating_mul(strings.len().saturating_sub(1)),
                );
            if size > MAX_BYTES {
                return Err(PlatformError::unsupported(
                    "String.join exceeds host allocation bound",
                ));
            }
            Value::Str(strings.join(separator))
        }
        ("string", "format") => {
            arity(args, 2)?;
            let Value::List(values) = &args[1] else {
                return Err(PlatformError::unsupported("String.format requires a List"));
            };
            Value::Str(message_format(text(&args[0])?, &values.borrow())?)
        }
        ("blob", "valueof") => {
            arity(args, 1)?;
            let b = text(&args[0])?.as_bytes();
            bounded(b)?;
            Value::Blob(b.to_vec())
        }
        ("crypto", "getrandominteger") => {
            arity(args, 0)?;
            let b = random(4)?;
            Value::Num(Number::integer(
                i32::from_ne_bytes(b.try_into().unwrap()) as i64
            ))
        }
        ("crypto", "getrandomlong") => {
            arity(args, 0)?;
            let b = random(8)?;
            let mut n = Number::integer(i64::from_ne_bytes(b.try_into().unwrap()));
            n.kind = NumKind::Long;
            Value::Num(n)
        }
        ("crypto", "generateaeskey") => {
            arity(args, 1)?;
            let bits = integer(&args[0])?;
            if ![128, 192, 256].contains(&bits) {
                return Err(PlatformError::exception(
                    "InvalidParameterValue",
                    "AES key size must be 128, 192, or 256",
                ));
            }
            Value::Blob(random(bits as usize / 8)?)
        }
        ("crypto", "generatedigest") => {
            arity(args, 2)?;
            Value::Blob(digest(text(&args[0])?, bytes(&args[1])?)?)
        }
        ("crypto", "generatemac") => {
            arity(args, 3)?;
            Value::Blob(mac(text(&args[0])?, bytes(&args[1])?, bytes(&args[2])?)?)
        }
        ("crypto", "verifyhmac") => {
            arity(args, 4)?;
            let m = mac(text(&args[0])?, bytes(&args[1])?, bytes(&args[2])?)?;
            Value::Bool(bool::from(m.as_slice().ct_eq(bytes(&args[3])?)))
        }
        ("crypto", "areequalconstanttime") => {
            arity(args, 2)?;
            Value::Bool(bool::from(bytes(&args[0])?.ct_eq(bytes(&args[1])?)))
        }
        ("crypto", "encrypt" | "decrypt") => {
            arity(args, 4)?;
            Value::Blob(aes(
                text(&args[0])?,
                bytes(&args[1])?,
                bytes(&args[2])?,
                bytes(&args[3])?,
                method == "decrypt",
            )?)
        }
        ("crypto", "encryptwithmanagediv") => {
            arity(args, 3)?;
            let iv = random(16)?;
            let encrypted = aes(
                text(&args[0])?,
                bytes(&args[1])?,
                &iv,
                bytes(&args[2])?,
                false,
            )?;
            Value::Blob([iv, encrypted].concat())
        }
        ("crypto", "decryptwithmanagediv") => {
            arity(args, 3)?;
            let data = bytes(&args[2])?;
            if data.len() < 16 {
                return Err(PlatformError::exception(
                    "InvalidParameterValue",
                    "Unable to parse the initialization vector from encrypted data.",
                ));
            }
            Value::Blob(aes(
                text(&args[0])?,
                bytes(&args[1])?,
                &data[..16],
                &data[16..],
                true,
            )?)
        }
        ("crypto", "sign") => {
            arity(args, 3)?;
            Value::Blob(sign(text(&args[0])?, bytes(&args[1])?, bytes(&args[2])?)?)
        }
        ("crypto", "verify") => {
            arity(args, 4)?;
            Value::Bool(verify(
                text(&args[0])?,
                bytes(&args[1])?,
                bytes(&args[2])?,
                bytes(&args[3])?,
            )?)
        }
        ("encodingutil", "base64encode") => {
            arity(args, 1)?;
            Value::Str(STANDARD.encode(bytes(&args[0])?))
        }
        ("encodingutil", "base64decode") => {
            arity(args, 1)?;
            let s = text(&args[0])?;
            if s.len() > MAX_BYTES * 2 {
                return Err(PlatformError::unsupported(
                    "Encoded data exceeds host bound",
                ));
            }
            let clean: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            let result = STANDARD
                .decode(clean)
                .map_err(|_| PlatformError::exception("StringException", "Invalid base64 data"))?;
            bounded(&result)?;
            Value::Blob(result)
        }
        ("encodingutil", "converttohex") => {
            arity(args, 1)?;
            Value::Str(hex::encode(bytes(&args[0])?))
        }
        ("encodingutil", "convertfromhex") => {
            arity(args, 1)?;
            let s = text(&args[0])?;
            if s.len() > MAX_BYTES * 2 {
                return Err(PlatformError::unsupported("Hex data exceeds host bound"));
            }
            Value::Blob(hex::decode(s).map_err(|_| {
                PlatformError::exception("StringException", "Invalid hexadecimal data")
            })?)
        }
        ("encodingutil", "urlencode" | "urldecode") => {
            arity(args, 2)?;
            if !["utf-8", "utf8"].contains(&text(&args[1])?.to_ascii_lowercase().as_str()) {
                return Err(PlatformError::unsupported(
                    "Only UTF-8 URL encoding is implemented",
                ));
            }
            Value::Str(if method == "urlencode" {
                url_encode(text(&args[0])?)?
            } else {
                url_decode(text(&args[0])?)?
            })
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

pub fn instance(receiver: &Value, method: &str, args: &[Value]) -> P<Option<Value>> {
    let method = method.to_ascii_lowercase();
    let value = match (receiver, method.as_str()) {
        (Value::Str(s), "tostring") => {
            arity(args, 0)?;
            Value::Str(s.clone())
        }
        (Value::Str(s), "compareto") => {
            arity(args, 1)?;
            let other = text(&args[0])?;
            let mut a = s.encode_utf16();
            let mut b = other.encode_utf16();
            let difference = loop {
                match (a.next(), b.next()) {
                    (Some(x), Some(y)) if x != y => break i64::from(x) - i64::from(y),
                    (Some(_), Some(_)) => {}
                    (None, None) => break 0,
                    _ => {
                        break s.encode_utf16().count() as i64 - other.encode_utf16().count() as i64
                    }
                }
            };
            Value::Num(Number::integer(difference))
        }
        (Value::Str(s), "containsignorecase" | "startswithignorecase" | "endswithignorecase") => {
            arity(args, 1)?;
            let other = text(&args[0])?;
            if !s.is_ascii() || !other.is_ascii() {
                return Err(PlatformError::unsupported(
                    "Non-ASCII case mapping requires qualified Unicode policy",
                ));
            }
            let s = s.to_ascii_lowercase();
            let other = other.to_ascii_lowercase();
            Value::Bool(match method.as_str() {
                "containsignorecase" => s.contains(&other),
                "startswithignorecase" => s.starts_with(&other),
                _ => s.ends_with(&other),
            })
        }
        (Value::Str(s), "substringbefore" | "substringafter") => {
            arity(args, 1)?;
            let separator = text(&args[0])?;
            Value::Str(if method == "substringbefore" {
                s.split_once(separator).map(|(a, _)| a).unwrap_or(s).into()
            } else {
                s.split_once(separator).map(|(_, b)| b).unwrap_or("").into()
            })
        }
        (Value::Str(s), "right" | "left") => {
            arity(args, 1)?;
            let count = integer(&args[0])?.max(0) as usize;
            let chars = s.encode_utf16().collect::<Vec<_>>();
            let selected = if method == "right" {
                &chars[chars.len().saturating_sub(count)..]
            } else {
                &chars[..count.min(chars.len())]
            };
            Value::Str(String::from_utf16(selected).map_err(|_| {
                PlatformError::unsupported(
                    "Unpaired UTF-16 string result requires UTF-16 value support",
                )
            })?)
        }
        (Value::Str(s), "indexof" | "lastindexof") => {
            arity(args, 1)?;
            let needle = text(&args[0])?;
            let index = if method == "indexof" {
                s.find(needle)
            } else {
                s.rfind(needle)
            };
            Value::Num(Number::integer(
                index
                    .map(|i| s[..i].encode_utf16().count() as i64)
                    .unwrap_or(-1),
            ))
        }
        (Value::Str(s), "remove") => {
            arity(args, 1)?;
            Value::Str(s.replace(text(&args[0])?, ""))
        }
        (Value::Str(s), "split" | "replaceall") => {
            arity(args, if method == "split" { 1 } else { 2 })?;
            let pattern = text(&args[0])?;
            crate::regex_subset::validate_input(pattern, s).map_err(PlatformError::unsupported)?;
            if pattern.len() > 4096 {
                return Err(PlatformError::unsupported(
                    "Regex exceeds 4096-byte host bound",
                ));
            }
            let re =
                crate::regex_subset::compile(pattern, false).map_err(PlatformError::unsupported)?;
            if method == "split" {
                if re.is_match("") || re.find_iter(s).any(|matched| matched.is_empty()) {
                    return Err(PlatformError::unsupported(
                        "Zero-width Java regex splitting requires qualified semantics",
                    ));
                }
                let mut parts = re
                    .split(s)
                    .map(|s| Value::Str(s.into()))
                    .collect::<Vec<_>>();
                if !s.is_empty() {
                    while matches!(parts.last(),Some(Value::Str(s))if s.is_empty()) {
                        parts.pop();
                    }
                }
                Value::list(parts)
            } else {
                let replacement = text(&args[1])?;
                if replacement.contains(['$', '\\']) {
                    return Err(PlatformError::unsupported("Regex replacement group/escape semantics require a Java-compatible replacement parser"));
                }
                // Rust omits empty matches adjacent to a nonempty match while
                // Java replaceAll includes them. Conservatively reject all
                // potentially empty patterns and observed empty matches.
                if re.is_match("") || re.find_iter(s).any(|matched| matched.is_empty()) {
                    return Err(PlatformError::unsupported(
                        "Zero-width Java regex replacement requires qualified semantics",
                    ));
                }
                let matches = re.find_iter(s).count();
                if matches
                    .saturating_mul(replacement.len())
                    .saturating_add(s.len())
                    > MAX_BYTES
                {
                    return Err(PlatformError::unsupported(
                        "Regex replacement exceeds host allocation bound",
                    ));
                }
                Value::Str(re.replace_all(s, replacement).into_owned())
            }
        }
        (Value::Blob(b), "size") => {
            arity(args, 0)?;
            Value::Num(Number::integer(b.len() as i64))
        }
        (Value::Blob(b), "tostring") => {
            arity(args, 0)?;
            Value::Str(String::from_utf8(b.clone()).map_err(|_| {
                PlatformError::exception("StringException", "BLOB is not a valid UTF-8 string")
            })?)
        }
        (Value::Str(s), "substring") => {
            if args.is_empty() || args.len() > 2 {
                return Err(PlatformError::unsupported(
                    "String.substring expects one or two Integer arguments",
                ));
            }
            let start = integer(&args[0])?;
            let chars: Vec<u16> = s.encode_utf16().collect();
            let end = if args.len() == 2 {
                integer(&args[1])?
            } else {
                chars.len() as i64
            };
            if start < 0 || end < start || end > chars.len() as i64 {
                return Err(PlatformError::exception(
                    "StringException",
                    "Ending or starting position out of bounds",
                ));
            }
            Value::Str(
                String::from_utf16(&chars[start as usize..end as usize]).map_err(|_| {
                    PlatformError::unsupported(
                        "Unpaired UTF-16 string values require the UTF-16 value implementation",
                    )
                })?,
            )
        }
        (Value::Str(s), "replace") => {
            arity(args, 2)?;
            let from = text(&args[0])?;
            let to = text(&args[1])?;
            let count = if from.is_empty() {
                s.chars().count() + 1
            } else {
                s.matches(from).count()
            };
            if count.saturating_mul(to.len()).saturating_add(s.len()) > MAX_BYTES {
                return Err(PlatformError::unsupported(
                    "String replacement exceeds host allocation bound",
                ));
            }
            Value::Str(s.replace(from, to))
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}
fn url_encode(s: &str) -> P<String> {
    bounded(s.as_bytes())?;
    let mut result = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                result.push(b as char)
            }
            b' ' => result.push('+'),
            _ => result.push_str(&format!("%{b:02X}")),
        }
        if result.len() > MAX_BYTES {
            return Err(PlatformError::unsupported(
                "URL output exceeds host allocation bound",
            ));
        }
    }
    Ok(result)
}
fn message_format(pattern: &str, args: &[Value]) -> P<String> {
    bounded(pattern.as_bytes())?;
    let chars = pattern.chars().collect::<Vec<_>>();
    let mut output = String::new();
    let mut i = 0;
    let mut quoted = false;
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                if chars.get(i + 1) == Some(&'\'') {
                    output.push('\'');
                    i += 1;
                } else {
                    quoted = !quoted;
                }
            }
            '{' if !quoted => {
                let start = i + 1;
                let Some(end) = (start..chars.len()).find(|j| chars[*j] == '}') else {
                    return Err(PlatformError::exception(
                        "StringException",
                        "Unmatched format brace",
                    ));
                };
                let index = chars[start..end]
                    .iter()
                    .collect::<String>()
                    .parse::<usize>()
                    .map_err(|_| {
                        PlatformError::unsupported(
                            "Typed MessageFormat patterns are not implemented",
                        )
                    })?;
                let value = args.get(index).ok_or_else(|| {
                    PlatformError::exception("StringException", "Format argument is missing")
                })?;
                output.push_str(text(value)?);
                i = end;
            }
            c => output.push(c),
        }
        if output.len() > MAX_BYTES {
            return Err(PlatformError::unsupported(
                "Formatted string exceeds host allocation bound",
            ));
        }
        i += 1;
    }
    Ok(output)
}
fn url_decode(s: &str) -> P<String> {
    bounded(s.as_bytes())?;
    let input = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < input.len() {
        match input[i] {
            b'+' => out.push(b' '),
            b'%' => {
                if i + 2 >= input.len() {
                    return Err(PlatformError::exception(
                        "StringException",
                        "Incomplete URL escape",
                    ));
                }
                let pair = &input[i + 1..i + 3];
                out.extend(hex::decode(pair).map_err(|_| {
                    PlatformError::exception("StringException", "Invalid URL escape")
                })?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out)
        .map_err(|_| PlatformError::exception("StringException", "URL data is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey};
    fn s(s: &str) -> Value {
        Value::Str(s.into())
    }
    fn b(b: &[u8]) -> Value {
        Value::Blob(b.into())
    }
    #[test]
    fn digest_and_hmac_standard_vectors() {
        assert_eq!(
            hex::encode(digest("SHA-256", b"abc").unwrap()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex::encode(mac("HmacSHA256", b"Hi There", &[0x0b; 20]).unwrap()),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }
    #[test]
    fn aes_cbc_pkcs7_known_vector_and_invalid_padding() {
        let plain = b"hello world! this is my plaintext.";
        let cipher = aes("AES128", &[0x42; 16], &[0x24; 16], plain, false).unwrap();
        assert_eq!(hex::encode(&cipher), "c7fe247ef97b21f07cbdd26cb5d346bfd27867cb00d9486723e159978fb9a5f914cfb228a710de4171e396e7b6cf859e");
        assert_eq!(
            aes("AES128", &[0x42; 16], &[0x24; 16], &cipher, true).unwrap(),
            plain
        );
        assert!(aes("AES128", &[0x42; 16], &[0x24; 16], &[0; 17], true).is_err());
    }
    #[test]
    fn managed_iv_is_a_real_prefix_and_round_trips() {
        let args = [s("AES256"), b(&[7; 32]), b(b"message")];
        let first = invoke("crypto", "encryptWithManagedIV", &args)
            .unwrap()
            .unwrap();
        let second = invoke("crypto", "encryptWithManagedIV", &args)
            .unwrap()
            .unwrap();
        assert_ne!(bytes(&first).unwrap(), bytes(&second).unwrap());
        let plain = invoke(
            "crypto",
            "decryptWithManagedIV",
            &[s("AES256"), b(&[7; 32]), first],
        )
        .unwrap()
        .unwrap();
        assert_eq!(bytes(&plain).unwrap(), b"message");
    }
    #[test]
    fn encoding_binary_and_utf8_bounds() {
        let decoded = invoke("encodingutil", "base64decode", &[s("AAEC\n/w==")])
            .unwrap()
            .unwrap();
        assert_eq!(bytes(&decoded).unwrap(), &[0, 1, 2, 255]);
        assert!(instance(&decoded, "tostring", &[]).is_err());
        assert_eq!(url_decode(&url_encode("a+b ☃").unwrap()).unwrap(), "a+b ☃");
        assert!(url_decode("bad%QZ").is_err());
    }
    #[test]
    fn hmac_verification_rejects_changed_data_and_signature() {
        let tag = mac("HMACSHA512", b"message", b"key").unwrap();
        let ok = invoke(
            "crypto",
            "verifyHMAC",
            &[s("HMACSHA512"), b(b"message"), b(b"key"), b(&tag)],
        )
        .unwrap()
        .unwrap();
        assert!(matches!(ok, Value::Bool(true)));
        let bad = invoke(
            "crypto",
            "verifyHMAC",
            &[s("HMACSHA512"), b(b"other"), b(b"key"), b(&tag)],
        )
        .unwrap()
        .unwrap();
        assert!(matches!(bad, Value::Bool(false)));
    }
    #[test]
    fn rsa_signature_checks_message_key_and_pkcs8_format() {
        let private = RsaPrivateKey::new(&mut OsRng, 1024).unwrap();
        let public = private.to_public_key().to_public_key_der().unwrap();
        let der = private.to_pkcs8_der().unwrap();
        let signature = sign("RSA-SHA256", b"message", der.as_bytes()).unwrap();
        assert!(verify("RSA-SHA256", b"message", &signature, public.as_bytes()).unwrap());
        assert!(!verify("RSA-SHA256", b"changed", &signature, public.as_bytes()).unwrap());
        let mut damaged = signature;
        damaged[0] ^= 1;
        assert!(!verify("RSA-SHA256", b"message", &damaged, public.as_bytes()).unwrap());
        assert!(sign("RSA-SHA256", b"message", b"not a DER key").is_err());
    }
    #[test]
    fn unknown_algorithm_and_unknown_api_do_not_fabricate_values() {
        assert!(invoke("crypto", "invented", &[]).unwrap().is_none());
        assert!(digest("invented", b"hello").unwrap_err().unsupported);
        assert!(
            sign("ECDSA-SHA256", b"message", b"key")
                .unwrap_err()
                .unsupported
        );
    }
    #[test]
    fn string_comparison_uses_utf16_and_format_uses_actual_arguments() {
        assert_eq!(
            instance(&s("az"), "compareto", &[s("aa")])
                .unwrap()
                .unwrap()
                .display(),
            "25"
        );
        assert_eq!(
            instance(&s("a"), "compareto", &[s("abcd")])
                .unwrap()
                .unwrap()
                .display(),
            "-3"
        );
        assert_eq!(
            instance(&s("😀"), "compareto", &[s("\u{e000}")])
                .unwrap()
                .unwrap()
                .display(),
            "-1987"
        );
        let formatted = invoke(
            "string",
            "format",
            &[s("Item {1}: {0}"), Value::list(vec![s("pear"), s("7")])],
        )
        .unwrap()
        .unwrap();
        assert_eq!(formatted.display(), "Item 7: pear");
        assert_eq!(
            instance(&s("prefix:suffix"), "substringafter", &[s(":")])
                .unwrap()
                .unwrap()
                .display(),
            "suffix"
        );
        assert!(
            instance(&s("abc"), "replaceall", &[s("(a)"), s("$1")])
                .unwrap_err()
                .unsupported
        );
    }
}
