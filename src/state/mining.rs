use super::*;
use crate::{
    error::PoolingError,
    math::{Decimal, TryAdd, TryMul, TrySub}
};
use arrayref::{array_mut_ref, array_refs, array_ref,mut_array_refs};
use solana_program::{
    entrypoint::ProgramResult,
    msg,
    program_error::{ProgramError},
    program_pack::{Pack, Sealed,IsInitialized},
    pubkey::{Pubkey, PUBKEY_BYTES},
};
use std::{
    convert::{TryFrom},
};



//Max number of (deposit + collateral + borrow)-related reserves in a mining position
pub const MAX_MINING_VOLUME: usize = 10;

/// Lending market mining state (used for un-collaterized portion of LToken the user holds)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mining {
    /// Version of the struct
    pub version: u8,
    /// Owner to whom this mining state instance belong
    pub owner: Pubkey,
    /// Lending market address
    pub pool_manager: Pubkey,
    /// A struct to hold a bunch of mining data, with each element representing a specific LToken's mining
    pub mining_indices:Vec<MiningIndex>,
    /// Total un-claimed mine for this user's all kinds of un-collaterized LTokens' mining.
    pub unclaimed_mine: Decimal,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MiningIndex{
    /// From which reserve this LToken is minted from.
    pub reserve:Pubkey,
    /// Un-collaterized amount of this LToken
    pub un_coll_l_token_amount:u64,
    /// User's mining index of this portion of (un-collaterized) LToken the user has accumulated to
    pub index:Decimal,
}
impl MiningIndex {
    /// Create new obligation collateral
    pub fn new(reserve: Pubkey,l_token_mining_index:Decimal) -> Self {
        Self {
            reserve,
            index: l_token_mining_index,
            un_coll_l_token_amount:0 as u64,
        }
    }
}
impl Mining {
    pub fn new(params: InitMiningParams) -> Self {
        let mut mining = Self::default();
        Self::init(&mut mining, params);
        mining
    }

    pub fn init(&mut self, params: InitMiningParams) {
        self.version = PROGRAM_VERSION;
        self.pool_manager = params.lending_market;
        self.owner = params.owner;
        self.unclaimed_mine = Decimal::zero();
        self.mining_indices = params.mining_indices;
    }

    /// Accrue mine for the user from the reserve in context (only for the portion of un-collaterized LToken)
    pub fn refresh_unclaimed(&mut self, index:usize, reserve:&Pool) -> ProgramResult{
        let mining_index = &mut self.mining_indices[index];
        self.unclaimed_mine = self.unclaimed_mine.try_add(
            reserve.lottery.l_token_mining_index
                .try_sub(mining_index.index)?
                .try_mul(mining_index.un_coll_l_token_amount)?
        )?;
        self.mining_indices[index].index = reserve.lottery.l_token_mining_index;
        Ok(())
    }
    pub fn find_mining_index(&mut self, reserve: &Pubkey)
     -> Result<usize, ProgramError> {
        if self.mining_indices.is_empty() {
            msg!("Mining position has no reserve yet.");
            return Err(PoolingError::MiningReserveEmpty.into());
        }
        let reserve_index = self._find_index_in_mining_indices(*reserve).ok_or(PoolingError::InvalidMiningReserve)?;

        Ok(reserve_index)
    }
    fn _find_index_in_mining_indices(&self, reserve: Pubkey) -> Option<usize> {
        self.mining_indices
            .iter()
            .position(|mining_index| mining_index.reserve == reserve)
    }
    pub fn find_or_add_reserve_in_vec(&mut self,reserve: Pubkey,l_token_mining_index:Decimal)
                                      -> Result<(&MiningIndex,usize), ProgramError> {
        if let Some(mining_index) = self._find_index_in_mining_indices(reserve) {
            return Ok((&self.mining_indices[mining_index],mining_index));
        }
        if self.mining_indices.len() >= MAX_MINING_VOLUME {
            msg!(
                "Mining cannot have more than {} deposits, collaterals, borrows combined",
                MAX_OBLIGATION_RESERVES
            );
            return Err(PoolingError::MiningVolumeLimit.into());
        }
        self.mining_indices.push(MiningIndex::new(reserve,l_token_mining_index));
        Ok((self.mining_indices.last().unwrap(),self.mining_indices.len()-1))
    }


    // Increase un-collaterized LToken
    pub fn deposit(&mut self, index: usize, amount: u64)
                   -> ProgramResult {
        let mining_index = &mut self.mining_indices[index];
        mining_index.un_coll_l_token_amount = mining_index.un_coll_l_token_amount.checked_add(amount).ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }

    // Decrease un-collaterized LToken
    pub fn withdraw(&mut self, index: usize, amount: u64)
                    -> ProgramResult {
        let mining_index = &mut self.mining_indices[index];
        if amount == mining_index.un_coll_l_token_amount{
            self.mining_indices.remove(index);
        } else {
            mining_index.un_coll_l_token_amount = mining_index.un_coll_l_token_amount.checked_sub(amount).ok_or(PoolingError::MathOverflow)?;
        }
        Ok(())
    }
}

const MINING_LEN: usize = 642;  //1+8+1+32+32+1+8+ 10*56
const MINING_INDEX_LEN: usize = 56;// 32+8+16
impl Pack for Mining {
    const LEN: usize = MINING_LEN;

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let output = array_mut_ref![dst, 0, MINING_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
        let (
            version,
            owner,
            lending_market,
            reserves_len,
            unclaimed_mine,
            data_flat,
        ) = mut_array_refs![
            output,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            1,
            16,
            MAX_MINING_VOLUME * MINING_INDEX_LEN
        ];
        *version = self.version.to_le_bytes();
        owner.copy_from_slice(self.owner.as_ref());
        lending_market.copy_from_slice(self.pool_manager.as_ref());
        *reserves_len = u8::try_from(self.mining_indices.len()).unwrap().to_le_bytes();        //what does unwrap() do here?
        pack_decimal(self.unclaimed_mine, unclaimed_mine);


        let mut offset = 0;
        //reserves
        for mining_index in &self.mining_indices {
            let mining_index_flat = array_mut_ref![data_flat, offset, MINING_INDEX_LEN];
            #[allow(clippy::ptr_offset_with_cast)]
                let (
                reserve_id,
                un_coll_l_token_amount,
                index
            ) = mut_array_refs![mining_index_flat, PUBKEY_BYTES,8,16];
            *un_coll_l_token_amount = mining_index.un_coll_l_token_amount.to_le_bytes();
            pack_decimal(mining_index.index,index);
            reserve_id.copy_from_slice(mining_index.reserve.as_ref());
            offset += MINING_INDEX_LEN;
        }
    }


    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let input = array_ref![src, 0, MINING_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            owner,
            lending_market,
            reserves_len,
            unclaimed_mine,
            data_flat,
        ) = array_refs![
            input,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            1,
            16,
            MAX_MINING_VOLUME * (MINING_INDEX_LEN)
        ];


        let version = u8::from_le_bytes(*version);
        if version > PROGRAM_VERSION {
            msg!("Obligation version does not match lending program version");
            return Err(ProgramError::InvalidAccountData);
        }

        let reserves_len = u8::from_le_bytes(*reserves_len);
        let mut mining_indices = Vec::with_capacity(reserves_len as usize + 1);
        let mut offset = 0;
        for _ in 0..reserves_len {
            let mining_index_flat = array_ref![data_flat, offset, MINING_INDEX_LEN];
            #[allow(clippy::ptr_offset_with_cast)]
                let (
                reserve,
                un_coll_l_token_amount,
                index
            ) = array_refs![mining_index_flat,PUBKEY_BYTES,8,16];

            mining_indices.push(MiningIndex{

                reserve: Pubkey::new(reserve),
                un_coll_l_token_amount: u64::from_le_bytes(*un_coll_l_token_amount),
                index: unpack_decimal(index)
            });
            offset += MINING_INDEX_LEN;
        }

        Ok(Self {
            version,
            owner: Pubkey::new_from_array(*owner),
            pool_manager: Pubkey::new_from_array(*lending_market),
            mining_indices,
            unclaimed_mine: unpack_decimal(unclaimed_mine),
        })
    }
}
impl IsInitialized for Mining{
    fn is_initialized(&self) -> bool {
        self.version != UNINITIALIZED_VERSION
    }
}
impl Sealed for Mining {}

pub struct InitMiningParams{
    pub lending_market: Pubkey,
    pub owner: Pubkey,
    pub mining_indices: Vec<MiningIndex>
}
