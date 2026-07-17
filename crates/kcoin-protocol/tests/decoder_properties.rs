use kcoin_protocol::{Block, CommitCertificate, SignedTransaction};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_transaction_bytes_fail_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..8_192)) {
        let _ = SignedTransaction::decode(&bytes);
    }

    #[test]
    fn arbitrary_block_bytes_fail_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..16_384)) {
        let _ = Block::decode(&bytes);
    }

    #[test]
    fn arbitrary_certificate_bytes_fail_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..8_192)) {
        let _ = CommitCertificate::decode(&bytes);
    }
}
