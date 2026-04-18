use data_encoding::BASE32_NOPAD;
use ed25519_dalek::SigningKey;

fn main() {
    // Generate raw ed25519 keypair
    let mut seed = [0u8; 32];
    let mut rng = rand::thread_rng();
    rand::RngCore::fill_bytes(&mut rng, &mut seed);

    let signing_key = SigningKey::from_bytes(&seed);
    let public_key: [u8; 32] = signing_key.verifying_key().to_bytes();

    println!("\n--- SYNOD COORDINATOR KEYS ---");
    println!("Add these to your .env file:\n");
    println!(
        "SYNOD_STELLAR__COORDINATOR_PUBKEY={}",
        encode_stellar_address(0x30, &public_key)
    );
    println!(
        "SYNOD_STELLAR__COORDINATOR_SECRET_KEY={}",
        encode_stellar_address(0x40, &seed)
    );
    println!("");
}

fn encode_stellar_address(version_byte: u8, data: &[u8; 32]) -> String {
    let mut payload = Vec::with_capacity(35);
    payload.push(version_byte);
    payload.extend_from_slice(data);

    let checksum = crc16_xmodem(&payload);
    payload.extend_from_slice(&checksum.to_le_bytes());

    BASE32_NOPAD.encode(&payload)
}

fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if crc & 0x8000 != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}
