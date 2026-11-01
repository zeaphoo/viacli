//! AES-128-ECB 加密工具

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyInit};
use aes::Aes128;

/// 使用 AES-128-ECB 加密数据（带 PKCS7 填充）
pub fn encrypt_aes_ecb(plaintext: &[u8], key: &[u8]) -> Vec<u8> {
    let mut cipher = Aes128::new_from_slice(key).expect("Invalid key length");
    let block_size = 16;
    let padded_size = aes_ecb_padded_size(plaintext.len());
    let mut padded = vec![0u8; padded_size];
    padded[..plaintext.len()].copy_from_slice(plaintext);

    // PKCS7 填充
    let padding_len = padded_size - plaintext.len();
    for i in plaintext.len()..padded_size {
        padded[i] = padding_len as u8;
    }

    // 逐块加密
    let mut ciphertext = vec![0u8; padded_size];
    for (i, chunk) in padded.chunks(block_size).enumerate() {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        cipher.encrypt_block_mut((&mut block).into());
        ciphertext[i * block_size..(i + 1) * block_size].copy_from_slice(&block);
    }

    ciphertext
}

/// 使用 AES-128-ECB 解密数据（带 PKCS7 填充去除）
pub fn decrypt_aes_ecb(ciphertext: &[u8], key: &[u8]) -> Vec<u8> {
    let mut cipher = Aes128::new_from_slice(key).expect("Invalid key length");
    let block_size = 16;
    let mut plaintext = vec![0u8; ciphertext.len()];

    // 逐块解密
    for (i, chunk) in ciphertext.chunks(block_size).enumerate() {
        let mut block = [0u8; 16];
        block.copy_from_slice(chunk);
        cipher.decrypt_block_mut((&mut block).into());
        plaintext[i * block_size..(i + 1) * block_size].copy_from_slice(&block);
    }

    // 去除 PKCS7 填充
    if let Some(&padding_len) = plaintext.last() {
        let padding_len = padding_len as usize;
        if padding_len > 0 && padding_len <= block_size {
            let data_len = plaintext.len() - padding_len;
            if plaintext[data_len..].iter().all(|&b| b == padding_len as u8) {
                plaintext.truncate(data_len);
            }
        }
    }

    plaintext
}

/// 计算 AES-128-ECB 密文大小（PKCS7 填充到 16 字节边界）
pub fn aes_ecb_padded_size(plaintext_size: usize) -> usize {
    // PKCS7 padding always adds at least 1 byte, and when plaintext_size is a
    // multiple of the block size, it adds a full extra block.
    // Therefore ciphertext length is always (floor(n/16) + 1) * 16.
    ((plaintext_size / 16) + 1) * 16
}

/// 生成随机 AES-128 密钥
pub fn generate_aes_key() -> [u8; 16] {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut key = [0u8; 16];
    rng.fill(&mut key);
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt() {
        let key = [0u8; 16];
        let plaintext = b"Hello, World!";
        let ciphertext = encrypt_aes_ecb(plaintext, &key);
        let decrypted = decrypt_aes_ecb(&ciphertext, &key);
        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn test_padded_size() {
        assert_eq!(aes_ecb_padded_size(0), 16);
        assert_eq!(aes_ecb_padded_size(1), 16);
        assert_eq!(aes_ecb_padded_size(15), 16);
        assert_eq!(aes_ecb_padded_size(16), 32);
        assert_eq!(aes_ecb_padded_size(17), 32);
    }
}
