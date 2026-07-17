use ed25519_dalek::SigningKey;
use kcoin_protocol::{Address, ChainId, SignedTransaction, TransactionAction, UnsignedTransaction};

fn main() {
    let sender_key = SigningKey::from_bytes(&[7; 32]);
    let recipient_key = SigningKey::from_bytes(&[9; 32]);
    let public_key = sender_key.verifying_key().to_bytes();
    let address = Address::from_public_key(&public_key);
    let recipient = Address::from_public_key(&recipient_key.verifying_key().to_bytes());
    let chain_id = ChainId::new("kcoin-local").unwrap();
    let transfer = UnsignedTransaction::new(
        chain_id.clone(),
        public_key,
        7,
        42,
        TransactionAction::Transfer {
            recipient,
            amount_atoms: 12_345_678,
        },
    );
    let signed_transfer = SignedTransaction::sign(transfer.clone(), &sender_key).unwrap();
    let display_name = UnsignedTransaction::new(
        chain_id,
        public_key,
        8,
        42,
        TransactionAction::SetDisplayName {
            display_name: Some("Ada".into()),
        },
    );
    let claim_reward = UnsignedTransaction::new(
        ChainId::new("kcoin-local").unwrap(),
        public_key,
        9,
        42,
        TransactionAction::ClaimReward {
            challenge_id: 3,
            answer: 12,
        },
    );

    println!("public_key_hex={}", hex::encode(public_key));
    println!("address={address}");
    println!("recipient={recipient}");
    println!(
        "transfer_signing_bytes_hex={}",
        hex::encode(transfer.signing_bytes())
    );
    println!(
        "transfer_signature_hex={}",
        hex::encode(signed_transfer.signature)
    );
    println!("transfer_id={}", signed_transfer.id());
    println!(
        "display_name_signing_bytes_hex={}",
        hex::encode(display_name.signing_bytes())
    );
    println!(
        "claim_reward_signing_bytes_hex={}",
        hex::encode(claim_reward.signing_bytes())
    );
}
