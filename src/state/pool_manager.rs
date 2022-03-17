use super::*;
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::{
    msg,
    program_error::ProgramError,
    program_pack::{IsInitialized, Pack, Sealed},
    pubkey::{Pubkey, PUBKEY_BYTES},
};

/// Lending market state
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoolManager {
    /// Version of pool manager
    pub version: u8,
    /// Bump seed for derived authority address
    pub bump_seed: u8,
    /// The pending owner
    pub pending_owner: Pubkey,
    /// Owner authority which can add new pool
    pub owner: Pubkey,
    /// Currency market prices are quoted in
    /// e.g. "USD" null padded (`*b"USD\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"`) or a SPL token mint pubkey
    pub quote_currency: [u8; 32],
    /// Token program id
    pub token_program_id: Pubkey,
    /// Oracle (Pyth) program id
    pub oracle_program_id: Pubkey,
    /// Mint address of the mine token
    pub mine_mint: Pubkey,
    /// Supply address of mine token
    pub mine_supply_account: Pubkey,

}

impl PoolManager {
    /// Create a new pool manager
    pub fn new(params: InitPoolManagerParams) -> Self {
        let mut pool_manager = Self::default();
        Self::init(&mut pool_manager, params);
        pool_manager
    }

    /// Initialize a pool manager
    pub fn init(&mut self, params: InitPoolManagerParams) {
        self.version = PROGRAM_VERSION;
        self.bump_seed = params.bump_seed;
        self.pending_owner = Pubkey::default();
        self.owner = params.owner;
        self.quote_currency = params.quote_currency;
        self.token_program_id = params.token_program_id;
        self.oracle_program_id = params.oracle_program_id;
        self.mine_mint = params.mine_mint;
        self.mine_supply_account = params.mine_supply_account;
    }
}

/// Initialize a lending market
pub struct InitPoolManagerParams {
    /// Bump seed for derived authority address
    pub bump_seed: u8,
    /// Owner authority which can add new reserves
    pub owner: Pubkey,
    /// Currency market prices are quoted in
    /// e.g. "USD" null padded (`*b"USD\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"`) or a SPL token mint pubkey
    pub quote_currency: [u8; 32],
    /// Token program id
    pub token_program_id: Pubkey,
    /// Oracle (Pyth) program id
    pub oracle_program_id: Pubkey,
    /// Mint address of the mine token
    pub mine_mint: Pubkey,
    /// Supply address of mine token
    pub mine_supply_account: Pubkey,

}

impl Sealed for PoolManager {}

impl IsInitialized for PoolManager {
    fn is_initialized(&self) -> bool {
        self.version != UNINITIALIZED_VERSION
    }
}

const POOL_MANAGER_LEN: usize = 354;

// 1 + 1 + 32 + 32 + 32 + 32 + 32 + 128
impl Pack for PoolManager {
    const LEN: usize = POOL_MANAGER_LEN;

    fn pack_into_slice(&self, output: &mut [u8]) {
        let output = array_mut_ref![output, 0, POOL_MANAGER_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            bump_seed,
            pending_owner,
            owner,
            quote_currency,
            token_program_id,
            oracle_program_id,
            mine_mint,
            mine_supply_account,
            _padding,
        ) = mut_array_refs![
            output,
            1,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            128
        ];

        *version = self.version.to_le_bytes();
        *bump_seed = self.bump_seed.to_le_bytes();
        owner.copy_from_slice(self.owner.as_ref());
        quote_currency.copy_from_slice(self.quote_currency.as_ref());
        token_program_id.copy_from_slice(self.token_program_id.as_ref());
        oracle_program_id.copy_from_slice(self.oracle_program_id.as_ref());
        pending_owner.copy_from_slice(self.pending_owner.as_ref());
        mine_mint.copy_from_slice(self.mine_mint.as_ref());
        mine_supply_account.copy_from_slice(self.mine_supply_account.as_ref());
    }

    /// Unpacks a byte buffer into a [PoolManagerInfo](struct.PoolManagerInfo.html)
    fn unpack_from_slice(input: &[u8]) -> Result<Self, ProgramError> {
        let input = array_ref![input, 0, POOL_MANAGER_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            bump_seed,
            pending_owner,
            owner,
            quote_currency,
            token_program_id,
            oracle_program_id,
            mine_mint,
            mine_supply_account,
            _padding,
        ) = array_refs![
            input,
            1,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            128
        ];

        let version = u8::from_le_bytes(*version);
        if version > PROGRAM_VERSION {
            msg!("pool manager version does not match pooling program version");
            return Err(ProgramError::InvalidAccountData);
        }

        Ok(Self {
            version,
            bump_seed: u8::from_le_bytes(*bump_seed),
            pending_owner: Pubkey::new_from_array(*pending_owner),
            owner: Pubkey::new_from_array(*owner),
            quote_currency: *quote_currency,
            token_program_id: Pubkey::new_from_array(*token_program_id),
            oracle_program_id: Pubkey::new_from_array(*oracle_program_id),
            mine_mint: Pubkey::new_from_array(*mine_mint),
            mine_supply_account: Pubkey::new_from_array(*mine_supply_account),
        })
    }
}
