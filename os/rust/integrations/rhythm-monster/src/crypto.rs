//! Ayla LAN v1: independent chained AES-256-CBC directions, HMAC-SHA256,
//! zero padding, monotonic authenticated sequence numbers. No key Debug output.
use crate::{LightError, LightResult};
use aes::{
    cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit},
    Aes256,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
type H = Hmac<Sha256>;
fn sign(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut h = <H as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary key length");
    h.update(data);
    h.finalize().into_bytes().to_vec()
}
fn derive(key: &[u8], seed: &str, suffix: char) -> Vec<u8> {
    let seed = format!("{seed}{suffix}").into_bytes();
    let mut input = sign(key, &seed);
    input.extend(seed);
    sign(key, &input)
}
struct Direction {
    aes: Aes256,
    iv: [u8; 16],
    sign: Vec<u8>,
}
impl Drop for Direction {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.sign.zeroize();
        self.iv.zeroize();
    }
}
impl Direction {
    fn new(key: &[u8], seed: &str) -> Self {
        Self {
            aes: Aes256::new(GenericArray::from_slice(&derive(key, seed, '1'))),
            iv: derive(key, seed, '2')[..16].try_into().unwrap(),
            sign: derive(key, seed, '0'),
        }
    }
}
pub struct LightLanCrypto {
    tx: Direction,
    rx: Direction,
    sequence: u64,
    received: Option<u64>,
}
impl LightLanCrypto {
    pub fn new(key: &str, r1: &str, r2: &str, t1: u64, t2: u64) -> LightResult<Self> {
        if r1.len() != 16 || r2.len() != 16 || !r1.is_ascii() || !r2.is_ascii() {
            return Err(LightError::Integrity);
        }
        Ok(Self {
            tx: Direction::new(key.as_bytes(), &format!("{r1}{r2}{t1}{t2}")),
            rx: Direction::new(key.as_bytes(), &format!("{r2}{r1}{t2}{t1}")),
            sequence: 0,
            received: None,
        })
    }
    pub fn encrypt(&mut self, data: Value) -> LightResult<Value> {
        let raw = serde_json::to_vec(&json!({"seq_no":self.sequence,"data":data}))
            .map_err(|_| LightError::Integrity)?;
        self.sequence = self.sequence.checked_add(1).ok_or(LightError::Integrity)?;
        let signature = STANDARD.encode(sign(&self.tx.sign, &raw));
        let mut out = raw;
        out.resize((out.len() / 16 + 1) * 16, 0);
        for chunk in out.chunks_exact_mut(16) {
            for (a, b) in chunk.iter_mut().zip(self.tx.iv) {
                *a ^= b;
            }
            self.tx
                .aes
                .encrypt_block(GenericArray::from_mut_slice(chunk));
            self.tx.iv.copy_from_slice(chunk);
        }
        Ok(json!({"enc":STANDARD.encode(out),"sign":signature}))
    }
    pub fn decrypt(&mut self, message: &Value) -> LightResult<Value> {
        let encoded = message["enc"].as_str().ok_or(LightError::Integrity)?;
        let signature = message["sign"].as_str().ok_or(LightError::Integrity)?;
        if encoded.len() > 87384 || signature.len() != 44 {
            return Err(LightError::Integrity);
        }
        let mut bytes = STANDARD
            .decode(encoded)
            .map_err(|_| LightError::Integrity)?;
        if bytes.is_empty() || bytes.len() > 65536 || bytes.len() % 16 != 0 {
            return Err(LightError::Integrity);
        }
        let signature = STANDARD
            .decode(signature)
            .map_err(|_| LightError::Integrity)?;
        let mut next_iv = self.rx.iv;
        for chunk in bytes.chunks_exact_mut(16) {
            let ciphertext: [u8; 16] = chunk.try_into().unwrap();
            self.rx
                .aes
                .decrypt_block(GenericArray::from_mut_slice(chunk));
            for (a, b) in chunk.iter_mut().zip(next_iv) {
                *a ^= b;
            }
            next_iv = ciphertext;
        }
        if bytes.last() != Some(&0) {
            return Err(LightError::Integrity);
        }
        while bytes.last() == Some(&0) {
            bytes.pop();
        }
        let mut verifier = <H as Mac>::new_from_slice(&self.rx.sign).unwrap();
        verifier.update(&bytes);
        verifier
            .verify_slice(&signature)
            .map_err(|_| LightError::Integrity)?;
        let plain: Value = serde_json::from_slice(&bytes).map_err(|_| LightError::Integrity)?;
        let sequence = plain["seq_no"].as_u64().ok_or(LightError::Integrity)?;
        if self.received.is_some_and(|old| sequence <= old) {
            return Err(LightError::Integrity);
        }
        self.rx.iv = next_iv;
        self.received = Some(sequence);
        Ok(plain["data"].clone())
    }
}
