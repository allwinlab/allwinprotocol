use super::*;
use crate::{
    error::PoolingError,
    math::{Decimal, Rate, TryAdd, TryDiv, TryMul, TrySub},
};
use arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs};
use solana_program::{
    clock::Slot,
    entrypoint::ProgramResult,
    msg,
    program_error::ProgramError,
    program_pack::{IsInitialized, Pack, Sealed},
    pubkey::{Pubkey, PUBKEY_BYTES},
};
use std::{
    cmp::Ordering,
    convert::{TryFrom, TryInto},
};


/// Max number of collateral and liquidity reserve accounts combined for an obligation
pub const MAX_OBLIGATION_RESERVES: usize = 10;

/// Lending market obligation state
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ticket {
    /// Version of the struct
    pub version: u8,
    /// Last update to collateral, liquidity, or their market values
    pub last_update: LastUpdate,
    /// Lending market address
    pub pool_manager: Pubkey,
    /// Owner authority which can borrow liquidity
    pub owner: Pubkey,
    /// Deposited collateral for the obligation, unique by deposit reserve address
    pub deposits: Vec<TicketCollateral>,
    /// Market value of deposits
    pub deposited_value: Decimal,
    /// Total unclaimed mine for the  in context
    pub unclaimed_mine: Decimal,
}

impl Ticket {
    /// Create a new obligation
    pub fn new(params: InitTicketParams) -> Self {
        let mut ticket = Self::default();
        Self::init(&mut ticket, params);
        ticket
    }

    /// Initialize an obligation
    pub fn init(&mut self, params: InitTicketParams) {
        self.version = PROGRAM_VERSION;
        self.last_update = LastUpdate::new(params.current_slot);
        self.pool_manager = params.pool_manager;
        self.owner = params.owner;
        self.deposits = params.deposits;
    }

    /// Accrue mine for this ticket account  from this reserve in context (only for the portion of collaterized LToken)
    pub fn refresh_deposit_unclaimed(&mut self, liquidity_index: usize, reserve: &Pool) -> ProgramResult {
        let liquidity = &mut self.deposits[liquidity_index];
        self.unclaimed_mine = self.unclaimed_mine.try_add(
            reserve.lottery.l_token_mining_index
                .try_sub(liquidity.index)?
                .try_mul(liquidity.deposited_amount)?
        )?;
        liquidity.index = reserve.lottery.l_token_mining_index;
        Ok(())
    }

    pub fn deposit(&mut self, index: usize, collateral_amount: u64) -> ProgramResult {
        let liquidity = &mut self.deposits[index];
        liquidity.deposit(collateral_amount)?;
        Ok(())
    }
    /// Withdraw collateral and remove it from deposits if zeroed out
    pub fn withdraw(&mut self, withdraw_amount: u64, collateral_index: usize) -> ProgramResult {
        let collateral = &mut self.deposits[collateral_index];
        if withdraw_amount == collateral.deposited_amount {
            self.deposits.remove(collateral_index);
        } else {
            collateral.withdraw(withdraw_amount)?;
        }
        Ok(())
    }


    /// Find collateral by deposit reserve
    pub fn find_collateral_in_deposits(
        &self,
        deposit_reserve: Pubkey,
    ) -> Result<(&TicketCollateral, usize), ProgramError> {
        if self.deposits.is_empty() {
            msg!("Obligation has no deposits");
            return Err(PoolingError::ObligationDepositsEmpty.into());
        }
        let collateral_index = self
            ._find_collateral_index_in_deposits(deposit_reserve)
            .ok_or(PoolingError::InvalidObligationCollateral)?;
        Ok((&self.deposits[collateral_index], collateral_index))
    }

    /// Find or add collateral by deposit reserve
    pub fn find_or_add_collateral_to_deposits(
        &mut self,
        deposit_reserve: Pubkey,
        l_token_mining_index: Decimal,
    ) -> Result<(&TicketCollateral, usize), ProgramError> {
        if let Some(collateral_index) = self._find_collateral_index_in_deposits(deposit_reserve) {
            return Ok((&self.deposits[collateral_index], collateral_index));
        }
        // if self.deposits.len() + self.borrows.len() >= MAX_OBLIGATION_RESERVES {
        if self.deposits.len() >= MAX_OBLIGATION_RESERVES {
            msg!(
                "Obligation cannot have more than {} deposits combined",
                MAX_OBLIGATION_RESERVES
            );
            return Err(PoolingError::ObligationReserveLimit.into());
        }
        let collateral = TicketCollateral::new(deposit_reserve, l_token_mining_index);
        self.deposits.push(collateral);
        Ok((self.deposits.last().unwrap(), self.deposits.len() - 1))
    }

    fn _find_collateral_index_in_deposits(&self, deposit_reserve: Pubkey) -> Option<usize> {
        self.deposits
            .iter()
            .position(|collateral| collateral.deposit_reserve == deposit_reserve)
    }
}

/// Initialize an obligation
pub struct InitTicketParams {
    /// Last update to collateral, liquidity, or their market values
    pub current_slot: Slot,
    /// Lending market address
    pub pool_manager: Pubkey,
    /// Owner authority which can borrow liquidity
    pub owner: Pubkey,
    /// Deposited collateral for the obligation, unique by deposit reserve address
    pub deposits: Vec<TicketCollateral>,
}

impl Sealed for Ticket {}

impl IsInitialized for Ticket {
    fn is_initialized(&self) -> bool {
        self.version != UNINITIALIZED_VERSION
    }
}

/// Obligation collateral state
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TicketCollateral {
    pub index: Decimal,
    /// Reserve collateral is deposited to
    pub deposit_reserve: Pubkey,
    /// Amount of collateral deposited
    pub deposited_amount: u64,
    /// Collateral market value in quote currency
    pub market_value: Decimal,
}

impl TicketCollateral {
    /// Create new obligation collateral
    pub fn new(deposit_reserve: Pubkey, l_token_mining_index: Decimal) -> Self {
        Self {
            index: l_token_mining_index,
            deposit_reserve,
            deposited_amount: 0 as u64,
            market_value: Decimal::zero(),
        }
    }

    /// Increase deposited collateral
    pub fn deposit(&mut self, collateral_amount: u64) -> ProgramResult {
        self.deposited_amount = self
            .deposited_amount
            .checked_add(collateral_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }

    /// Decrease deposited collateral
    pub fn withdraw(&mut self, collateral_amount: u64) -> ProgramResult {
        self.deposited_amount = self
            .deposited_amount
            .checked_sub(collateral_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }
}

/// Obligation liquidity state
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObligationLiquidity {
    pub index: Decimal,
    /// Reserve liquidity is borrowed from
    pub borrow_reserve: Pubkey,
    /// Borrow rate used for calculating interest
    pub cumulative_borrow_rate_wads: Decimal,
    /// Amount of liquidity borrowed plus interest
    pub borrowed_amount_wads: Decimal,
    /// Liquidity market value in quote currency
    pub market_value: Decimal,
}

impl ObligationLiquidity {
    /// Create new obligation liquidity
    pub fn new(borrow_reserve: Pubkey, cumulative_borrow_rate_wads: Decimal, borrow_mining_index: Decimal) -> Self {
        Self {
            index: borrow_mining_index,
            borrow_reserve,
            cumulative_borrow_rate_wads,
            borrowed_amount_wads: Decimal::zero(),
            market_value: Decimal::zero(),
        }
    }

    /// Decrease borrowed liquidity
    pub fn repay(&mut self, settle_amount: Decimal) -> ProgramResult {
        self.borrowed_amount_wads = self.borrowed_amount_wads.try_sub(settle_amount)?;
        Ok(())
    }

    /// Increase borrowed liquidity
    pub fn borrow(&mut self, borrow_amount: Decimal) -> ProgramResult {
        self.borrowed_amount_wads = self.borrowed_amount_wads.try_add(borrow_amount)?;
        Ok(())
    }

    /// Accrue interest
    pub fn accrue_interest(&mut self, cumulative_borrow_rate_wads: Decimal) -> ProgramResult {
        match cumulative_borrow_rate_wads.cmp(&self.cumulative_borrow_rate_wads) {
            Ordering::Less => {
                msg!("Interest rate cannot be negative");
                return Err(PoolingError::NegativeInterestRate.into());
            }
            Ordering::Equal => {}
            Ordering::Greater => {
                let compounded_interest_rate: Rate = cumulative_borrow_rate_wads
                    .try_div(self.cumulative_borrow_rate_wads)?
                    .try_into()?;
                self.borrowed_amount_wads = self
                    .borrowed_amount_wads
                    .try_mul(compounded_interest_rate)?;
                self.cumulative_borrow_rate_wads = cumulative_borrow_rate_wads;
            }
        }

        Ok(())
    }
}

const OBLIGATION_COLLATERAL_LEN: usize = 72;
// 32 + 8 + 16 + 16
const OBLIGATION_LEN: usize = 827; //107+720

impl Pack for Ticket {
    const LEN: usize = OBLIGATION_LEN;

    fn pack_into_slice(&self, dst: &mut [u8]) {
        let output = array_mut_ref![dst, 0, OBLIGATION_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            last_update_slot,
            last_update_stale,
            pool_manager,
            owner,
            deposited_value,
            deposits_len,
            unclaimed_mine,
            data_flat,
        ) = mut_array_refs![
            output,
            1, // version
            8, // last_update_slot
            1, // last_update_stale
            PUBKEY_BYTES, // pool_manager
            PUBKEY_BYTES, // owner
            16, // deposited_value
            1, // deposits_len
            16, // unclaimed_mine
            OBLIGATION_COLLATERAL_LEN * MAX_OBLIGATION_RESERVES
        ];

        // obligation
        *version = self.version.to_le_bytes();
        *last_update_slot = self.last_update.slot.to_le_bytes();
        pack_bool(self.last_update.stale, last_update_stale);
        pool_manager.copy_from_slice(self.pool_manager.as_ref());
        owner.copy_from_slice(self.owner.as_ref());
        pack_decimal(self.deposited_value, deposited_value);
        *deposits_len = u8::try_from(self.deposits.len()).unwrap().to_le_bytes();
        pack_decimal(self.unclaimed_mine, unclaimed_mine);
        let mut offset = 0;
        // deposits
        for collateral in &self.deposits {
            let deposits_flat = array_mut_ref![data_flat, offset, OBLIGATION_COLLATERAL_LEN];
            #[allow(clippy::ptr_offset_with_cast)]
                let (
                deposit_reserve,
                deposited_amount,
                market_value,
                index
            ) = mut_array_refs![deposits_flat, PUBKEY_BYTES, 8, 16,16];
            deposit_reserve.copy_from_slice(collateral.deposit_reserve.as_ref());
            *deposited_amount = collateral.deposited_amount.to_le_bytes();
            pack_decimal(collateral.market_value, market_value);
            pack_decimal(collateral.index, index);
            offset += OBLIGATION_COLLATERAL_LEN;
        }
    }

    /// Unpacks a byte buffer into an [ObligationInfo](struct.ObligationInfo.html).
    fn unpack_from_slice(src: &[u8]) -> Result<Self, ProgramError> {
        let input = array_ref![src, 0, OBLIGATION_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            last_update_slot,
            last_update_stale,
            pool_manager,
            owner,
            deposited_value,
            deposits_len,
            unclaimed_mine,
            data_flat,
        ) = array_refs![
            input,
            1,
            8,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            16,
            1,
            16,
            OBLIGATION_COLLATERAL_LEN * MAX_OBLIGATION_RESERVES
        ];

        let version = u8::from_le_bytes(*version);
        if version > PROGRAM_VERSION {
            msg!("Ticket version does not match lending program version");
            return Err(ProgramError::InvalidAccountData);
        }
        let deposits_len = u8::from_le_bytes(*deposits_len);
        let mut deposits = Vec::with_capacity(deposits_len as usize + 1);
        let mut offset = 0;
        for _ in 0..deposits_len {
            let deposits_flat = array_ref![data_flat, offset, OBLIGATION_COLLATERAL_LEN];
            #[allow(clippy::ptr_offset_with_cast)]
                let (
                deposit_reserve,
                deposited_amount,
                market_value,
                index
            ) = array_refs![deposits_flat, PUBKEY_BYTES, 8, 16, 16];
            deposits.push(TicketCollateral {
                index: unpack_decimal(index),
                deposit_reserve: Pubkey::new(deposit_reserve),
                deposited_amount: u64::from_le_bytes(*deposited_amount),
                market_value: unpack_decimal(market_value),
            });

            offset += OBLIGATION_COLLATERAL_LEN;
        }
        Ok(Self {
            version,
            last_update: LastUpdate {
                slot: u64::from_le_bytes(*last_update_slot),
                stale: unpack_bool(last_update_stale)?,
            },
            pool_manager: Pubkey::new_from_array(*pool_manager),
            owner: Pubkey::new_from_array(*owner),
            deposits,
            deposited_value: unpack_decimal(deposited_value),
            unclaimed_mine: unpack_decimal(unclaimed_mine),
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::math::TryAdd;
    use proptest::prelude::*;

    const MAX_COMPOUNDED_INTEREST: u64 = 100; // 10,000%

    #[test]
    fn obligation_accrue_interest_failure() {
        assert_eq!(
            ObligationLiquidity {
                cumulative_borrow_rate_wads: Decimal::zero(),
                ..ObligationLiquidity::default()
            }
                .accrue_interest(Decimal::one()),
            Err(PoolingError::MathOverflow.into())
        );

        assert_eq!(
            ObligationLiquidity {
                cumulative_borrow_rate_wads: Decimal::from(2u64),
                ..ObligationLiquidity::default()
            }
                .accrue_interest(Decimal::one()),
            Err(PoolingError::NegativeInterestRate.into())
        );

        assert_eq!(
            ObligationLiquidity {
                cumulative_borrow_rate_wads: Decimal::one(),
                borrowed_amount_wads: Decimal::from(u64::MAX),
                ..ObligationLiquidity::default()
            }
                .accrue_interest(Decimal::from(10 * MAX_COMPOUNDED_INTEREST)),
            Err(PoolingError::MathOverflow.into())
        );
    }

    // Creates rates (r1, r2) where 0 < r1 <= r2 <= 100*r1
    prop_compose! {
        fn cumulative_rates()(rate in 1..=u128::MAX)(
            current_rate in Just(rate),
            max_new_rate in rate..=rate.saturating_mul(MAX_COMPOUNDED_INTEREST as u128),
        ) -> (u128, u128) {
            (current_rate, max_new_rate)
        }
    }

    const MAX_BORROWED: u128 = u64::MAX as u128 * WAD as u128;

    // Creates liquidity amounts (repay, borrow) where repay < borrow
    prop_compose! {
        fn repay_partial_amounts()(amount in 1..=u64::MAX)(
            repay_amount in Just(WAD as u128 * amount as u128),
            borrowed_amount in (WAD as u128 * amount as u128 + 1)..=MAX_BORROWED,
        ) -> (u128, u128) {
            (repay_amount, borrowed_amount)
        }
    }

    // Creates liquidity amounts (repay, borrow) where repay >= borrow
    prop_compose! {
        fn repay_full_amounts()(amount in 1..=u64::MAX)(
            repay_amount in Just(WAD as u128 * amount as u128),
        ) -> (u128, u128) {
            (repay_amount, repay_amount)
        }
    }

    proptest! {
        #[test]
        fn repay_partial(
            (repay_amount, borrowed_amount) in repay_partial_amounts(),
        ) {
            let borrowed_amount_wads = Decimal::from_scaled_val(borrowed_amount);
            let repay_amount_wads = Decimal::from_scaled_val(repay_amount);
            println!("borrowed_amount_wads=,repay_amount_wads");
            println!("{}",borrowed_amount_wads);
            println!("{}",repay_amount_wads);
            let mut obligation = Ticket {
                borrows: vec![ObligationLiquidity {
                    borrowed_amount_wads,
                    ..ObligationLiquidity::default()
                }],
                ..Ticket::default()
            };

            obligation.repay(repay_amount_wads, 0)?;
            assert!(obligation.borrows[0].borrowed_amount_wads < borrowed_amount_wads);
            assert!(obligation.borrows[0].borrowed_amount_wads > Decimal::zero());
        }

        #[test]
        fn repay_full(
            (repay_amount, borrowed_amount) in repay_full_amounts(),
        ) {
            let borrowed_amount_wads = Decimal::from_scaled_val(borrowed_amount);
            let repay_amount_wads = Decimal::from_scaled_val(repay_amount);
            let mut obligation = Ticket {
                borrows: vec![ObligationLiquidity {
                    borrowed_amount_wads,
                    ..ObligationLiquidity::default()
                }],
                ..Ticket::default()
            };

            obligation.repay(repay_amount_wads, 0)?;
            assert_eq!(obligation.borrows.len(), 0);
        }

        #[test]
        fn accrue_interest(
            (current_borrow_rate, new_borrow_rate) in cumulative_rates(),
            borrowed_amount in 0..=u64::MAX,
        ) {
            let cumulative_borrow_rate_wads = Decimal::one().try_add(Decimal::from_scaled_val(current_borrow_rate))?;
            let borrowed_amount_wads = Decimal::from(borrowed_amount);
            let mut liquidity = ObligationLiquidity {
                cumulative_borrow_rate_wads,
                borrowed_amount_wads,
                ..ObligationLiquidity::default()
            };

            let next_cumulative_borrow_rate = Decimal::one().try_add(Decimal::from_scaled_val(new_borrow_rate))?;
            liquidity.accrue_interest(next_cumulative_borrow_rate)?;

            if next_cumulative_borrow_rate > cumulative_borrow_rate_wads {
                assert!(liquidity.borrowed_amount_wads > borrowed_amount_wads);
            } else {
                assert!(liquidity.borrowed_amount_wads == borrowed_amount_wads);
            }
        }
    }
}
