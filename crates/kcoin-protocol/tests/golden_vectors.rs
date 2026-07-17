use ed25519_dalek::SigningKey;
use kcoin_protocol::{Address, ChainId, SignedTransaction, TransactionAction, UnsignedTransaction};

const PUBLIC_KEY_HEX: &str = "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c";
const ADDRESS: &str = "kcoin1k7kkmlf8ljc67nnpg55n7h298ze8sepptjyscr";
const RECIPIENT: &str = "kcoin1dzhfuwwte062wyyudfmjtw2kwcl58dhal34n8a";
const TRANSFER_SIGNING_BYTES_HEX: &str = "4b434f494e5f54585f56310001000b0000006b636f696e2d6c6f63616cea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c07000000000000002a000000000000000068ae9e39cbcbf4a7109c6a7725b956763f43b6fd4e61bc0000000000";
const TRANSFER_SIGNATURE_HEX: &str = "dd0d99794938b981310c1846a0c721cf6f89f65c22d00a3a2be944970540c77a02875c1120a24bad9a8afb9d347b4fc3d7d8913090340d4050e0efd1eb49a00b";
const TRANSFER_ID: &str = "457e03358299b7f27541fe6a08d3b08055ea4149cfd7b16f03fe5ed617678bbd";
const DISPLAY_NAME_SIGNING_BYTES_HEX: &str = "4b434f494e5f54585f56310001000b0000006b636f696e2d6c6f63616cea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c08000000000000002a00000000000000020103000000416461";
const CLAIM_REWARD_SIGNING_BYTES_HEX: &str = "4b434f494e5f54585f56310001000b0000006b636f696e2d6c6f63616cea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c09000000000000002a000000000000000103000000000000000c00";

#[test]
fn rust_matches_checked_in_browser_wallet_vectors() {
    let sender_key = SigningKey::from_bytes(&[7; 32]);
    let recipient_key = SigningKey::from_bytes(&[9; 32]);
    let public_key = sender_key.verifying_key().to_bytes();
    assert_eq!(hex::encode(public_key), PUBLIC_KEY_HEX);
    assert_eq!(Address::from_public_key(&public_key).to_string(), ADDRESS);

    let recipient = Address::from_public_key(&recipient_key.verifying_key().to_bytes());
    assert_eq!(recipient.to_string(), RECIPIENT);
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
    assert_eq!(
        hex::encode(transfer.signing_bytes()),
        TRANSFER_SIGNING_BYTES_HEX
    );
    let signed_transfer = SignedTransaction::sign(transfer, &sender_key).unwrap();
    assert_eq!(
        hex::encode(signed_transfer.signature),
        TRANSFER_SIGNATURE_HEX
    );
    assert_eq!(signed_transfer.id().to_string(), TRANSFER_ID);

    let display_name = UnsignedTransaction::new(
        chain_id,
        public_key,
        8,
        42,
        TransactionAction::SetDisplayName {
            display_name: Some("Ada".into()),
        },
    );
    assert_eq!(
        hex::encode(display_name.signing_bytes()),
        DISPLAY_NAME_SIGNING_BYTES_HEX
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
    assert_eq!(
        hex::encode(claim_reward.signing_bytes()),
        CLAIM_REWARD_SIGNING_BYTES_HEX
    );

    // Keeping this artifact in the test dependency graph makes accidental
    // deletion visible even though the assertions stay serde-independent.
    assert!(include_str!("../test-vectors/wallet.json").contains(PUBLIC_KEY_HEX));
}
