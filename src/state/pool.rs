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

pub mod init_pool_accounts_index {
    ///   0. `[writable]` Reserve account - uninitialized.
    pub const RESERVE_ACCOUNT: usize = 0 as usize;
    ///   1. `[]` Reserve liquidity SPL Token mint.
    pub const LIQUIDITY_MINT: usize = 1 as usize;
    ///   2. `[]` Reserve liquidity supply SPL Token account.
    pub const LIQUIDITY_SUPPLY: usize = 2 as usize;
    ///   3. `[]` Reserve liquidity fee receiver.
    pub const LIQUIDITY_FEE_RECEIVER: usize = 3 as usize;
    ///   4. `[]` Pyth product account.
    pub const PYTH_PRODUCT: usize = 4 as usize;
    ///   5. `[]` Pyth price account.
    ///             This will be used as the reserve liquidity oracle account.
    pub const PYTH_PRICE: usize = 5 as usize;
    ///   7. `[]` Reserve collateral SPL Token mint.
    pub const COLLATERAL_MINT: usize = 6 as usize;
    ///   8. `[]` Reserve collateral token supply.
    pub const COLLATERAL_SUPPLY: usize = 7 as usize;
    ///   9  `[]` Lending market account.
    pub const POOL_MANAGER: usize = 8 as usize;
    ///   10  `[signer]` Lending market owner.
    pub const POOL_MANAGER_OWNER: usize = 9 as usize;
    ///   11. `[]` Un_coll_supply_account
    pub const UN_COLL_SUPPLY: usize = 10 as usize;
    ///   12  `[]` Clock sysvar.
    pub const CLOCK_SYSVAR: usize = 11 as usize;
    ///   13 `[]` Rent sysvar.
    pub const RENT_SYSVAR: usize = 12 as usize;
    ///   14 `[]` Token program id.
    pub const TOKEN_PROGRAM_ID: usize = 13 as usize;
}


/// pool's state
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pool {
    /// Version of the struct
    pub version: u8,
    /// Last slot when supply and rates updated
    pub last_update: LastUpdate,
    /// pool manager address
    pub pool_manager: Pubkey,
    /// Reserve liquidity
    pub liquidity: ReserveLiquidity,
    /// Reserve collateral
    pub collateral: ReserveCollateral,
    /// Reserve configuration values
    pub config: PoolConfig,
    /// Bonus (used for storing mining-info of a reserve)
    pub lottery: Lottery,
    /// Entry lock
    pub reentry_lock: bool,
}

impl Pool {
    /// Create a new pool
    pub fn new(params: InitPoolParams) -> Self {
        let mut pool = Self::default();
        Self::init(&mut pool, params);
        pool
    }

    /// Initialize a reserve
    pub fn init(&mut self, params: InitPoolParams) {
        self.version = PROGRAM_VERSION;
        self.last_update = LastUpdate::new(params.current_slot);
        self.pool_manager = params.pool_manager;
        self.liquidity = params.liquidity;
        self.collateral = params.collateral;
        self.config = params.config;
        self.lottery = params.lottery;
        self.reentry_lock = false;
    }
    pub fn refresh_index(&mut self, slot: Slot) -> ProgramResult {
        if self.collateral.mint_total_supply == 0 {
            return Ok(());
        }
        // let lend_side_mine_ratio: Rate = Rate::one();
        let (lend_side_mine_ratio, borrow_side_mine_ratio) = self.get_mine_ratio()?;
        self.lottery.l_token_mining_index = self.lottery.l_token_mining_index.try_add(
            Decimal::from(self.lottery.total_mining_speed)
                .try_mul(slot.checked_sub(self.last_update.slot).ok_or(PoolingError::MathOverflow)?)?
                .try_mul(lend_side_mine_ratio)?
                .try_div(self.collateral.mint_total_supply)?
        )?;

        let original_share = self.liquidity.borrowed_amount_wads
            .try_div(self.liquidity.cumulative_borrow_rate_wads)?;
        if original_share.lt(&Decimal::one()) {
            return Ok(());
        }
        self.lottery.borrow_mining_index = self.lottery.borrow_mining_index.try_add(
            Decimal::from(self.lottery.total_mining_speed)
                .try_mul(slot.checked_sub(self.last_update.slot).ok_or(PoolingError::MathOverflow)?)?
                .try_mul(borrow_side_mine_ratio)?
                .try_div(original_share)?
        )?;
        Ok(())
    }
    ///
    /// 挖矿比例
    fn get_mine_ratio(&self) -> Result<(Rate, Rate), ProgramError> {
        Ok((Rate::one().try_div(Rate::from_percent(50))?, Rate::one().try_div(Rate::from_percent(50))?))
        // if self.collateral.mint_total_supply == 0 as u64 {
        //     return Ok((Rate::zero(), Rate::zero()));
        // }
        // if self.liquidity.borrowed_amount_wads.lt(&Decimal::one()) {
        //     return Ok((Rate::one(), Rate::zero()));
        // }
        //
        // let utilization_rate = self.liquidity.utilization_rate()?;
        // let kink_rate = Rate::try_from(
        //     Decimal::from(self.lottery.kink_util_rate).try_div(Decimal::from(10000 as u64))?
        // )?;
        // if utilization_rate < kink_rate {
        //     let normalized_rate = utilization_rate.try_div(kink_rate)?;
        //     let min_rate = Rate::from_percent(0);
        //     let rate_range = Rate::from_percent(50);
        //     let mining_rate = normalized_rate.try_mul(rate_range)?.try_add(min_rate)?;
        //
        //     Ok((mining_rate, Rate::one().try_sub(mining_rate)?))
        // } else {
        //     let normalized_rate = utilization_rate
        //         .try_sub(kink_rate)?
        //         .try_div(Rate::from_percent(100u8).try_sub(kink_rate)?)?;
        //     let min_rate = Rate::from_percent(50);
        //     let rate_range = Rate::from_percent(100u8).try_sub(min_rate)?;
        //     let mining_rate = normalized_rate.try_mul(rate_range)?.try_add(min_rate)?;
        //     Ok((mining_rate, Rate::one().try_sub(mining_rate)?))
        // }
    }

    /// Record deposited liquidity and return amount of collateral tokens to mint
    pub fn deposit_liquidity(&mut self, liquidity_amount: u64) -> Result<u64, ProgramError> {
        let collateral_amount = self
            .collateral_exchange_rate()?
            .liquidity_to_collateral(liquidity_amount)?;

        self.liquidity.deposit(liquidity_amount)?;
        self.collateral.mint(collateral_amount)?;

        Ok(collateral_amount)
    }

    /// Record redeemed collateral and return amount of liquidity to withdraw
    pub fn redeem_collateral(&mut self, collateral_amount: u64) -> Result<u64, ProgramError> {
        let collateral_exchange_rate = self.collateral_exchange_rate()?;
        let liquidity_amount =
            collateral_exchange_rate.collateral_to_liquidity(collateral_amount)?;

        self.collateral.burn(collateral_amount)?;
        self.liquidity.withdraw(liquidity_amount)?;

        Ok(liquidity_amount)
    }


    /// Collateral exchange rate
    pub fn collateral_exchange_rate(&self) -> Result<CollateralExchangeRate, ProgramError> {
        let total_liquidity = self.liquidity.total_supply()?;
        self.collateral.exchange_rate(total_liquidity)
    }

    // Check if host fee receiver the check_receiver is
    // pub fn is_host_fee_receiver(&self, check_receiver: &Pubkey) -> Result<bool, ProgramError> {
    //     Ok(self.config.fees.host_fee_receivers.contains(check_receiver))
    // }
}


/// Calculate borrow result
#[derive(Debug)]
pub struct CalculateBorrowResult {
    /// Total amount of borrow including fees
    pub borrow_amount: Decimal,
    /// Borrow amount portion of total amount
    pub receive_amount: u64,
    /// Loan origination fee
    pub borrow_fee: u64,
    /// Host fee portion of origination fee
    pub host_fee: u64,
}

/// Calculate repay result
#[derive(Debug)]
pub struct CalculateRepayResult {
    /// Amount of liquidity that is settled from the obligation.
    pub settle_amount: Decimal,
    /// Amount that will be repaid as u64
    pub repay_amount: u64,
}

/// Calculate liquidation result
#[derive(Debug)]
pub struct CalculateLiquidationResult {
    /// Amount of liquidity that is settled from the obligation. It includes
    /// the amount of loan that was defaulted if collateral is depleted.
    pub settle_amount: Decimal,
    /// Amount that will be repaid as u64
    pub repay_amount: u64,
    /// Amount of collateral to withdraw in exchange for repay amount
    pub withdraw_amount: u64,
}

/// Reserve liquidity
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReserveLiquidity {
    /// Reserve liquidity mint address
    pub mint_pubkey: Pubkey,
    /// Reserve liquidity mint decimals
    pub mint_decimals: u8,
    /// Reserve liquidity supply address
    pub supply_pubkey: Pubkey,
    /// Reserve liquidity fee receiver address
    pub fee_receiver: Pubkey,
    /// If use pyth oracle
    pub use_pyth_oracle: bool,
    /// Reserve liquidity pyth oracle account
    pub pyth_oracle_pubkey: Pubkey,
    /// Reserve liquidity available
    pub available_amount: u64,
    /// Reserve liquidity borrowed
    pub borrowed_amount_wads: Decimal,
    /// Reserve liquidity cumulative borrow rate
    pub cumulative_borrow_rate_wads: Decimal,
    /// Reserve liquidity market price in quote currency
    pub market_price: Decimal,
    /// unclaimed fee by reserve owner
    pub owner_unclaimed: Decimal,
}

impl ReserveLiquidity {
    /// Create a new reserve liquidity
    pub fn new(params: NewReserveLiquidityParams) -> Self {
        Self {
            mint_pubkey: params.mint_pubkey,
            mint_decimals: params.mint_decimals,
            supply_pubkey: params.supply_pubkey,
            fee_receiver: params.fee_receiver,
            use_pyth_oracle: params.use_pyth_oracle,
            pyth_oracle_pubkey: params.pyth_oracle_pubkey,
            // larix_oracle_pubkey: params.larix_oracle_pubkey,
            available_amount: 0,
            borrowed_amount_wads: Decimal::zero(),
            cumulative_borrow_rate_wads: Decimal::one(),
            market_price: params.market_price,
            owner_unclaimed: Decimal::zero(),
        }
    }

    /// Calculate the total reserve supply including active loans
    pub fn total_supply(&self) -> Result<Decimal, ProgramError> {
        let all_liquidity = Decimal::from(self.available_amount)
            .try_add(self.borrowed_amount_wads)?;
        if all_liquidity.lt(&self.owner_unclaimed) {
            Ok(Decimal::zero())
        } else {
            all_liquidity.try_sub(self.owner_unclaimed)
        }
        // all_liquidity.try_sub(self.owner_unclaimed)
    }

    /// Add liquidity to available amount
    pub fn deposit(&mut self, liquidity_amount: u64) -> ProgramResult {
        self.available_amount = self
            .available_amount
            .checked_add(liquidity_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }

    /// Remove liquidity from available amount
    pub fn withdraw(&mut self, liquidity_amount: u64) -> ProgramResult {
        if liquidity_amount > self.liquidity_amount()? {
            msg!("Withdraw amount cannot exceed (available_amount - owner_fee)");
            return Err(PoolingError::InsufficientLiquidity.into());
        }
        self.available_amount = self
            .available_amount
            .checked_sub(liquidity_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }
    /// Subtract borrow amount from available liquidity and add to borrows
    pub fn borrow(&mut self, borrow_decimal: Decimal) -> ProgramResult {
        if borrow_decimal.try_ceil_u64()? > self.liquidity_amount()? {
            msg!("Insufficient liquidity due to fee reserved for reserve owner");
            return Err(PoolingError::InsufficientLiquidity.into());
        }
        self.available_amount = self
            .available_amount
            .checked_sub(borrow_decimal.try_round_u64()?)
            .ok_or(PoolingError::MathOverflow)?;
        self.borrowed_amount_wads = self.borrowed_amount_wads.try_add(borrow_decimal)?;

        Ok(())
    }
    pub fn liquidity_amount(&self) -> Result<u64, ProgramError> {
        if Decimal::from(self.available_amount).lt(&self.owner_unclaimed) {
            Ok(0 as u64)
        } else {
            Ok(self.available_amount
                .checked_sub(self.owner_unclaimed.try_ceil_u64()?)
                .ok_or(PoolingError::MathOverflow)?
            )
        }
    }
    pub fn decimal_liquidity_amount(&self) -> Result<Decimal, ProgramError> {
        if Decimal::from(self.available_amount).lt(&self.owner_unclaimed) {
            Ok(Decimal::zero())
        } else {
            Decimal::from(self.available_amount).try_sub(self.owner_unclaimed)
        }
    }


    /// Add repay amount to available liquidity and subtract settle amount from total borrows
    pub fn repay(&mut self, repay_amount: u64, settle_amount: Decimal) -> ProgramResult {
        self.available_amount = self
            .available_amount
            .checked_add(repay_amount)
            .ok_or(PoolingError::MathOverflow)?;
        self.borrowed_amount_wads = self.borrowed_amount_wads.try_sub(settle_amount)?;

        Ok(())
    }

    /// Calculate the liquidity utilization rate of the reserve
    pub fn utilization_rate(&self) -> Result<Rate, ProgramError> {
        let total_supply = self.total_supply()?;
        if total_supply == Decimal::zero() {
            return Ok(Rate::zero());
        }
        if self.borrowed_amount_wads.lt(&Decimal::one()) {
            return Ok(Rate::zero());
        }
        if self.borrowed_amount_wads.gt(&total_supply) {
            Ok(Rate::one())
        } else {
            self.borrowed_amount_wads.try_div(total_supply)?.try_into()
        }
    }

    /// Compound current borrow rate over elapsed slots
    fn compound_interest(
        &mut self,
        current_borrow_rate: Rate,
        slots_elapsed: u64,
        reserve_owner_fee_wad: u64,
    ) -> ProgramResult {
        let slot_interest_rate = current_borrow_rate.try_div(SLOTS_PER_YEAR)?;
        let compounded_interest_rate = Rate::one()
            .try_add(slot_interest_rate)?
            .try_pow(slots_elapsed)?;
        self.cumulative_borrow_rate_wads = self
            .cumulative_borrow_rate_wads
            .try_mul(compounded_interest_rate)?;
        let new_unclaimed = self.borrowed_amount_wads
            .try_mul(compounded_interest_rate.try_sub(Rate::one())?)?
            .try_mul(Rate::from_scaled_val(reserve_owner_fee_wad))?;
        self.owner_unclaimed = self
            .owner_unclaimed
            .try_add(new_unclaimed)?;

        self.borrowed_amount_wads = self
            .borrowed_amount_wads
            .try_mul(compounded_interest_rate)?;

        Ok(())
    }
}

/// Create a new reserve liquidity
pub struct NewReserveLiquidityParams {
    /// Reserve liquidity mint address
    pub mint_pubkey: Pubkey,
    /// Reserve liquidity mint decimals
    pub mint_decimals: u8,
    /// Reserve liquidity supply address
    pub supply_pubkey: Pubkey,
    /// Reserve liquidity fee receiver address
    pub fee_receiver: Pubkey,
    /// If use pyth oracle
    pub use_pyth_oracle: bool,
    /// Reserve liquidity pyth oracle account
    pub pyth_oracle_pubkey: Pubkey,
    /// Reserve liquidity larix oracle account
    // pub larix_oracle_pubkey: Pubkey,
    /// Reserve liquidity market price in quote currency
    pub market_price: Decimal,
}

/// Reserve collateral
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReserveCollateral {
    /// Reserve collateral mint address
    pub mint_pubkey: Pubkey,
    /// Reserve collateral mint supply, used for exchange rate
    pub mint_total_supply: u64,
    /// Reserve collateral supply address
    pub supply_pubkey: Pubkey,
}

impl ReserveCollateral {
    /// Create a new reserve collateral
    pub fn new(params: NewReserveCollateralParams) -> Self {
        Self {
            mint_pubkey: params.mint_pubkey,
            mint_total_supply: 0,
            supply_pubkey: params.supply_pubkey,
        }
    }

    /// Add collateral to total supply
    pub fn mint(&mut self, collateral_amount: u64) -> ProgramResult {
        self.mint_total_supply = self
            .mint_total_supply
            .checked_add(collateral_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }

    /// Remove collateral from total supply
    pub fn burn(&mut self, collateral_amount: u64) -> ProgramResult {
        self.mint_total_supply = self
            .mint_total_supply
            .checked_sub(collateral_amount)
            .ok_or(PoolingError::MathOverflow)?;
        Ok(())
    }

    /// Return the current collateral exchange rate.
    fn exchange_rate(
        &self,
        total_liquidity: Decimal,
    ) -> Result<CollateralExchangeRate, ProgramError> {
        let rate = if self.mint_total_supply == 0 || total_liquidity == Decimal::zero() {
            Rate::from_scaled_val(INITIAL_COLLATERAL_RATE)
        } else {
            let mint_total_supply = Decimal::from(self.mint_total_supply);
            Rate::try_from(mint_total_supply.try_div(total_liquidity)?)?
        };

        Ok(CollateralExchangeRate(rate))
    }
}

/// Create a new reserve collateral
pub struct NewReserveCollateralParams {
    /// Reserve collateral mint address
    pub mint_pubkey: Pubkey,
    /// Reserve collateral supply address
    pub supply_pubkey: Pubkey,
}

/// Collateral exchange rate
#[derive(Clone, Copy, Debug)]
pub struct CollateralExchangeRate(Rate);

impl CollateralExchangeRate {
    /// Convert reserve collateral to liquidity
    pub fn collateral_to_liquidity(&self, collateral_amount: u64) -> Result<u64, ProgramError> {
        Decimal::from(collateral_amount)
            .try_div(self.0)?
            .try_floor_u64()
    }

    /// Convert reserve collateral to liquidity
    pub fn decimal_collateral_to_liquidity(
        &self,
        collateral_amount: Decimal,
    ) -> Result<Decimal, ProgramError> {
        collateral_amount.try_div(self.0)
    }

    /// Convert reserve liquidity to collateral
    pub fn liquidity_to_collateral(&self, liquidity_amount: u64) -> Result<u64, ProgramError> {
        self.0.try_mul(liquidity_amount)?.try_round_u64()
    }

    /// Convert reserve liquidity to collateral
    pub fn decimal_liquidity_to_collateral(
        &self,
        liquidity_amount: Decimal,
    ) -> Result<Decimal, ProgramError> {
        liquidity_amount.try_mul(self.0)
    }
}

impl From<CollateralExchangeRate> for Rate {
    fn from(exchange_rate: CollateralExchangeRate) -> Self {
        exchange_rate.0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Copy)]
pub struct Lottery {
    /// Supply address of un-collaterized LToken
    pub un_coll_supply_account: Pubkey,
    /// Global mining index of this LToken
    pub l_token_mining_index: Decimal,
    /// Global mining index of borrowing in this reserve
    pub borrow_mining_index: Decimal,

    /// Amount of mine token for this reserve per slot
    pub total_mining_speed: u64,
    /// the critical liquidity utilization rate at which the mine distribution curve jumps
    pub kink_util_rate: u64,
}

pub struct InitBonusParams {
    pub un_coll_supply_account: Pubkey,
    pub total_mining_speed: u64,
    pub kink_util_rate: u64,
}

impl Lottery {
    pub fn new(params: InitBonusParams) -> Self {
        Self {
            un_coll_supply_account: params.un_coll_supply_account,
            l_token_mining_index: Decimal::zero(),
            borrow_mining_index: Decimal::zero(),
            total_mining_speed: params.total_mining_speed,
            kink_util_rate: params.kink_util_rate,
        }
    }
}

/// Initialize a reserve
pub struct InitPoolParams {
    /// Last slot when supply and rates updated
    pub current_slot: Slot,
    /// Lending market address
    pub pool_manager: Pubkey,
    /// Reserve liquidity
    pub liquidity: ReserveLiquidity,
    /// Reserve collateral
    pub collateral: ReserveCollateral,
    /// Reserve configuration values
    pub config: PoolConfig,
    /// Reserve bonus
    pub lottery: Lottery,
}

/// Reserve configuration values
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoolConfig {
    pub deposit_paused: bool,
}

/// Calculate fees exlusive or inclusive of an amount
pub enum FeeCalculation {
    /// Fee added to amount: fee = rate * amount
    Exclusive,
    /// Fee included in amount: fee = (rate / (1 + rate)) * amount
    Inclusive,
}

impl Sealed for Pool {}

impl IsInitialized for Pool {
    fn is_initialized(&self) -> bool {
        self.version != UNINITIALIZED_VERSION
    }
}

const RESERVE_LEN: usize = 646;

impl Pack for Pool {
    const LEN: usize = RESERVE_LEN;

    // @TODO: break this up by reserve / liquidity / collateral / config https://git.io/JOCca
    fn pack_into_slice(&self, output: &mut [u8]) {
        let output = array_mut_ref![output, 0, RESERVE_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            last_update_slot,
            last_update_stale,
            pool_manager,
            liquidity_mint_pubkey,
            liquidity_mint_decimals,
            liquidity_supply_pubkey,
            liquidity_fee_receiver,
            liquidity_use_pyth_oracle,
            liquidity_pyth_oracle_pubkey,
            liquidity_available_amount,
            liquidity_borrowed_amount_wads,
            liquidity_cumulative_borrow_rate_wads,
            liquidity_market_price,
            owner_unclaimed,
            collateral_mint_pubkey,
            collateral_mint_total_supply,
            collateral_supply_pubkey,
            deposit_paused,
            un_coll_supply_account,
            l_token_mining_index,
            borrow_mining_index,
            total_mining_speed,
            kink_util_rate,
            reentry_lock,
            _padding,
        ) = mut_array_refs![
               output,
            1,// version 1
            8,// last_update_slot 9
            1,// last_update_stale 10
            PUBKEY_BYTES,// for pool manager 42
            PUBKEY_BYTES,// liquidity_mint_pubkey   74
            1,// liquidity_mint_decimals    75
            PUBKEY_BYTES,// liquidity_supply_pubkey 107
            PUBKEY_BYTES,// liquidity_fee_receiver  139
            1,// liquidity_use_pyth_oracle  140
            PUBKEY_BYTES,// liquidity_pyth_oracle_pubkey 172
            8,// liquidity_available_amount 180
            16,// liquidity_borrowed_amount_wads 196
            16,// liquidity_cumulative_borrow_rate_wads 212
            16,// liquidity_market_price 228
            16,// owner_unclaimed 244
            PUBKEY_BYTES,// collateral_mint_pubkey 276
            8,// collateral_mint_total_supply 284
            PUBKEY_BYTES,// collateral_supply_pubkey 316
            1,// deposit_paused 317
            PUBKEY_BYTES,// un_coll_supply_account 349
            16,// l_token_mining_index 365
            16,// borrow_mining_index 381
            8,// total_mining_speed 389
            8,// kink_util_rate 397
            1, // reentry_lock  398
            248 //_padding 646
        ];

        // reserve
        *version = self.version.to_le_bytes();
        *last_update_slot = self.last_update.slot.to_le_bytes();
        pack_bool(self.last_update.stale, last_update_stale);
        pool_manager.copy_from_slice(self.pool_manager.as_ref());

        // liquidity
        liquidity_mint_pubkey.copy_from_slice(self.liquidity.mint_pubkey.as_ref());
        *liquidity_mint_decimals = self.liquidity.mint_decimals.to_le_bytes();
        liquidity_supply_pubkey.copy_from_slice(self.liquidity.supply_pubkey.as_ref());
        liquidity_fee_receiver.copy_from_slice(self.liquidity.fee_receiver.as_ref());
        pack_bool(self.liquidity.use_pyth_oracle, liquidity_use_pyth_oracle);
        liquidity_pyth_oracle_pubkey.copy_from_slice(self.liquidity.pyth_oracle_pubkey.as_ref());
        // liquidity_larix_oracle_pubkey.copy_from_slice(self.liquidity.larix_oracle_pubkey.as_ref());
        *liquidity_available_amount = self.liquidity.available_amount.to_le_bytes();
        pack_decimal(
            self.liquidity.borrowed_amount_wads,
            liquidity_borrowed_amount_wads,
        );
        pack_decimal(
            self.liquidity.cumulative_borrow_rate_wads,
            liquidity_cumulative_borrow_rate_wads,
        );
        pack_decimal(self.liquidity.market_price, liquidity_market_price);

        // collateral
        collateral_mint_pubkey.copy_from_slice(self.collateral.mint_pubkey.as_ref());
        *collateral_mint_total_supply = self.collateral.mint_total_supply.to_le_bytes();
        collateral_supply_pubkey.copy_from_slice(self.collateral.supply_pubkey.as_ref());

        pack_bool(self.config.deposit_paused, deposit_paused);

        un_coll_supply_account.copy_from_slice(self.lottery.un_coll_supply_account.as_ref());
        pack_decimal(self.lottery.l_token_mining_index, l_token_mining_index);
        pack_decimal(self.lottery.borrow_mining_index, borrow_mining_index);

        *total_mining_speed = self.lottery.total_mining_speed.to_le_bytes();
        *kink_util_rate = self.lottery.kink_util_rate.to_le_bytes();
        pack_decimal(self.liquidity.owner_unclaimed, owner_unclaimed);
        pack_bool(self.reentry_lock, reentry_lock);
    }

    /// Unpacks a byte buffer into a [ReserveInfo](struct.ReserveInfo.html).
    fn unpack_from_slice(input: &[u8]) -> Result<Self, ProgramError> {
        let input = array_ref![input, 0, RESERVE_LEN];
        #[allow(clippy::ptr_offset_with_cast)]
            let (
            version,
            last_update_slot,
            last_update_stale,
            pool_manager,
            liquidity_mint_pubkey,
            liquidity_mint_decimals,
            liquidity_supply_pubkey,
            liquidity_fee_receiver,
            liquidity_use_pyth_oracle,
            liquidity_pyth_oracle_pubkey,
            liquidity_available_amount,
            liquidity_borrowed_amount_wads,
            liquidity_cumulative_borrow_rate_wads,
            liquidity_market_price,
            owner_unclaimed,
            collateral_mint_pubkey,
            collateral_mint_total_supply,
            collateral_supply_pubkey,
            deposit_paused,
            un_coll_supply_account,
            l_token_mining_index,
            borrow_mining_index,
            total_mining_speed,
            kink_util_rate,
            reentry_lock,
            _padding,
        ) = array_refs![
            input,
            1,
            8,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            1,
            PUBKEY_BYTES,
            PUBKEY_BYTES,
            1,
            PUBKEY_BYTES,
            8,
            16,
            16,
            16,
            16,
            PUBKEY_BYTES,
            8,
            PUBKEY_BYTES,
            1,
            PUBKEY_BYTES,
            16,
            16,
            8,
            8,
            1,
            248
        ];

        let version = u8::from_le_bytes(*version);
        if version > PROGRAM_VERSION {
            msg!("Reserve version does not match pooling program version");
            return Err(ProgramError::InvalidAccountData);
        }

        Ok(Self {
            version,
            last_update: LastUpdate {
                slot: u64::from_le_bytes(*last_update_slot),
                stale: unpack_bool(last_update_stale)?,
            },
            pool_manager: Pubkey::new_from_array(*pool_manager),
            liquidity: ReserveLiquidity {
                mint_pubkey: Pubkey::new_from_array(*liquidity_mint_pubkey),
                mint_decimals: u8::from_le_bytes(*liquidity_mint_decimals),
                supply_pubkey: Pubkey::new_from_array(*liquidity_supply_pubkey),
                fee_receiver: Pubkey::new_from_array(*liquidity_fee_receiver),
                use_pyth_oracle: unpack_bool(liquidity_use_pyth_oracle)?,
                pyth_oracle_pubkey: Pubkey::new_from_array(*liquidity_pyth_oracle_pubkey),
                available_amount: u64::from_le_bytes(*liquidity_available_amount),
                borrowed_amount_wads: unpack_decimal(liquidity_borrowed_amount_wads),
                cumulative_borrow_rate_wads: unpack_decimal(liquidity_cumulative_borrow_rate_wads),
                market_price: unpack_decimal(liquidity_market_price),
                owner_unclaimed: unpack_decimal(owner_unclaimed),
            },
            collateral: ReserveCollateral {
                mint_pubkey: Pubkey::new_from_array(*collateral_mint_pubkey),
                mint_total_supply: u64::from_le_bytes(*collateral_mint_total_supply),
                supply_pubkey: Pubkey::new_from_array(*collateral_supply_pubkey),
            },
            config: PoolConfig {
                deposit_paused: unpack_bool(deposit_paused)?,
            },
            lottery: Lottery {
                un_coll_supply_account: Pubkey::new_from_array(*un_coll_supply_account),
                l_token_mining_index: unpack_decimal(l_token_mining_index),
                borrow_mining_index: unpack_decimal(borrow_mining_index),
                total_mining_speed: u64::from_le_bytes(*total_mining_speed),
                kink_util_rate: u64::from_le_bytes(*kink_util_rate),
            },
            reentry_lock: unpack_bool(reentry_lock)?,
        })
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::math::{PERCENT_SCALER, WAD};
    use proptest::prelude::*;
    use std::cmp::Ordering;

    const MAX_LIQUIDITY: u64 = u64::MAX / 5;

    // Creates rates (min, opt, max) where 0 <= min <= opt <= max <= MAX
    prop_compose! {
        fn borrow_rates()(optimal_rate in 1..=30 as u8)(
            min_rate in 0..=optimal_rate,
            optimal_rate in Just(optimal_rate),
            max_rate in optimal_rate..= 36 as u8,
        ) -> (u8, u8, u8) {
            (min_rate, optimal_rate, max_rate)
        }
    }

    // Creates rates (threshold, ltv) where 2 <= threshold <= 100 and threshold <= ltv <= 1,000%
    prop_compose! {
        fn unhealthy_rates()(threshold in 2..=100u8)(
            ltv_rate in threshold as u64..=1000u64,
            threshold in Just(threshold),
        ) -> (Decimal, u8) {
            (Decimal::from_scaled_val(ltv_rate as u128 * PERCENT_SCALER as u128), threshold)
        }
    }

    // Creates a range of reasonable token conversion rates
    prop_compose! {
        fn token_conversion_rate()(
            conversion_rate in 1..=u16::MAX,
            invert_conversion_rate: bool,
        ) -> Decimal {
            let conversion_rate = Decimal::from(conversion_rate as u64);
            if invert_conversion_rate {
                Decimal::one().try_div(conversion_rate).unwrap()
            } else {
                conversion_rate
            }
        }
    }

    // Creates a range of reasonable collateral exchange rates
    prop_compose! {
        fn collateral_exchange_rate_range()(percent in 1..=500u64) -> CollateralExchangeRate {
            CollateralExchangeRate(Rate::from_scaled_val(percent * PERCENT_SCALER))
        }
    }

    proptest! {
        #[test]
        fn total_supply(
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            owner_unclaimed_amount in 0..=u128::from(MAX_LIQUIDITY/100) * u128::from(WAD),
        ){
             let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);

             let liquidity:ReserveLiquidity = ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    ..ReserveLiquidity::default()
                };
            let total_supply = liquidity.total_supply()?;
            // println!("total_liquidity={},borrowed_percent={},borrowed_amount_wads={},owner_unclaimed={},total_supply={}",total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,owner_unclaimed,total_supply);
        }
        #[test]
        fn utilization_rate(
             total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            owner_unclaimed_amount in 0..=u128::from(MAX_LIQUIDITY/100) * u128::from(WAD),
        ){
              let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);

             let liquidity:ReserveLiquidity = ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    ..ReserveLiquidity::default()
                };
            let utilization_rate = liquidity.utilization_rate()?;
            // println!("total_liquidity={},borrowed_percent={},borrowed_amount_wads={},owner_unclaimed={},utilization_rate={}",total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,owner_unclaimed,utilization_rate);
        }
        #[test]
        fn get_mine_ratio(
            mint_total_supply in 0..=MAX_LIQUIDITY,
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            optimal_utilization_rate in 0..=100u8,
            owner_unclaimed_amount in 0..=u128::MAX / u128::from(u64::MAX) / 1000 as u128 * u128::from(WAD),
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),
        ){
            let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);
            let reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    mint_total_supply,
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };
            let (mining_ratio,borrow_ratio)=reserve.get_mine_ratio()?;
            // println!("mint_total_supply={},total_liquidity={},borrowed_percent={},borrowed_amount_wads={},owner_unclaimed={},mining_ratio={},borrow_ratio={}",
            //     mint_total_supply,total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,owner_unclaimed,mining_ratio,borrow_ratio);
        }
        #[test]
        fn refresh_index(
               mint_total_supply in 0..=MAX_LIQUIDITY,
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            optimal_utilization_rate in 0..=100u8,
            owner_unclaimed_amount in 0..=u128::MAX / u128::from(u64::MAX) / 1000 as u128 * u128::from(WAD),
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),
               cumulative_borrow_rate_wads in WAD..=WAD + WAD / 100000 ,
        ){

            let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let cumulative_borrow_rate_decimal = Decimal::from_scaled_val(u128::from(cumulative_borrow_rate_wads));
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);
            let mut reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    cumulative_borrow_rate_wads:cumulative_borrow_rate_decimal,
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    mint_total_supply,
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };
            reserve.refresh_index(100)?;
            // println!("mint_total_supply={},total_liquidity={},borrowed_percent={},borrowed_amount_wads={},cumulative_borrow_rate_decimal={},owner_unclaimed={},l_token_mining_index={},borrow_mining_index={}",
            //     mint_total_supply,total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,cumulative_borrow_rate_decimal,owner_unclaimed,reserve.bonus.l_token_mining_index,reserve.bonus.borrow_mining_index);
        }
        #[test]
        fn refresh_index_boundary(
               mint_total_supply in 0..=MAX_LIQUIDITY,
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            optimal_utilization_rate in 0..=100u8,
            owner_unclaimed_amount in 0..=u128::MAX / u128::from(u64::MAX) / 1000 as u128 * u128::from(WAD),
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),

        ){
           let cumulative_borrow_rate_wads  = 10*WAD;
            // let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let borrowed_amount_wads = Decimal::from_scaled_val(u128::from(WAD+1));

            let cumulative_borrow_rate_decimal = Decimal::from_scaled_val(u128::from(cumulative_borrow_rate_wads));
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);
            let mut reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    cumulative_borrow_rate_wads:cumulative_borrow_rate_decimal,
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    mint_total_supply,
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };
            reserve.refresh_index(100)?;
            // println!("mint_total_supply={},total_liquidity={},borrowed_percent={},borrowed_amount_wads={},cumulative_borrow_rate_decimal={},owner_unclaimed={},l_token_mining_index={},borrow_mining_index={}",
            //     mint_total_supply,total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,cumulative_borrow_rate_decimal,owner_unclaimed,reserve.bonus.l_token_mining_index,reserve.bonus.borrow_mining_index);
        }
        #[test]
        fn current_borrow_rate(
                mint_total_supply in 0..=MAX_LIQUIDITY,
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            optimal_utilization_rate in 0..=100u8,
            owner_unclaimed_amount in 0..=u128::MAX / u128::from(u64::MAX) / 1000 as u128 * u128::from(WAD),
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),
               cumulative_borrow_rate_wads in WAD..=WAD + WAD / 100000 ,
        ) {
            let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let cumulative_borrow_rate_decimal = Decimal::from_scaled_val(u128::from(cumulative_borrow_rate_wads));
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);
            let mut reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    owner_unclaimed,
                    cumulative_borrow_rate_wads:cumulative_borrow_rate_decimal,
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    mint_total_supply,
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };
            // println!("total_liquidity={},borrowed_percent={},borrowed_amount_wads={},optimal_utilization_rate={},owner_unclaimed_amount={},owner_unclaimed={},min_borrow_rate={},optimal_borrow_rate={},max_borrow_rate={}",
            //         total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_amount_wads,optimal_utilization_rate,owner_unclaimed_amount,owner_unclaimed,min_borrow_rate,optimal_borrow_rate,max_borrow_rate);
            let current_borrow_rate = reserve.current_borrow_rate()?;
            // println!("current_borrow_rate={}",current_borrow_rate);
            assert!(current_borrow_rate >= Rate::from_percent(min_borrow_rate));
            assert!(current_borrow_rate <= Rate::from_percent(max_borrow_rate));

            let optimal_borrow_rate = Rate::from_percent(optimal_borrow_rate);
            let current_rate = reserve.liquidity.utilization_rate()?;
            // println!("current_rate={}",current_rate);
            assert!(current_rate <= Rate::from_percent(100));
            match current_rate.cmp(&Rate::from_percent(optimal_utilization_rate)) {
                Ordering::Less => {
                    if min_borrow_rate == reserve.config.optimal_borrow_rate {
                        assert_eq!(current_borrow_rate, optimal_borrow_rate);
                    } else {
                        assert!(current_borrow_rate < optimal_borrow_rate);
                    }
                }
                Ordering::Equal => assert!(current_borrow_rate == optimal_borrow_rate),
                Ordering::Greater => {
                    if max_borrow_rate == reserve.config.optimal_borrow_rate {
                        assert_eq!(current_borrow_rate, optimal_borrow_rate);
                    } else {
                        assert!(current_borrow_rate > optimal_borrow_rate);
                    }
                }
            }
        }

        #[test]
        fn collateral_exchange_rate(
            total_liquidity in 0..=MAX_LIQUIDITY,
            borrowed_percent in 0..=WAD,
            collateral_multiplier in 0..=(5*WAD),
            borrow_rate in 0..=100u8,
            owner_unclaimed_amount in 0..= u128::MAX / u128::from(u64::MAX) / 1000u128 * u128::from(WAD),
            cumulative_borrow_rate_wads in WAD..=WAD + WAD / 100000 ,
        ) {
            let borrowed_liquidity_wads = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(borrowed_percent))?;
            let available_liquidity = total_liquidity - borrowed_liquidity_wads.try_round_u64()?;
            let mint_total_supply = Decimal::from(total_liquidity).try_mul(Rate::from_scaled_val(collateral_multiplier))?.try_round_u64()?;
             let cumulative_borrow_rate_decimal = Decimal::from_scaled_val(u128::from(cumulative_borrow_rate_wads));
            let owner_unclaimed = Decimal::from_scaled_val(owner_unclaimed_amount);
            let mut reserve = Pool {
                collateral: ReserveCollateral {
                    mint_total_supply,
                    ..ReserveCollateral::default()
                },
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads: borrowed_liquidity_wads,
                    available_amount: available_liquidity,
                    cumulative_borrow_rate_wads:cumulative_borrow_rate_decimal,
                    owner_unclaimed,
                    ..ReserveLiquidity::default()
                },
                config: PoolConfig {
                    min_borrow_rate: borrow_rate,
                    optimal_borrow_rate: borrow_rate,
                    optimal_utilization_rate: 100,
                    ..PoolConfig::default()
                },
                ..Pool::default()
            };
            if owner_unclaimed.gt(&Decimal::from(total_liquidity)){
                return Ok(());
            }
            let exchange_rate = reserve.collateral_exchange_rate()?;
            // assert!(exchange_rate.0.to_scaled_val() <= 5u128 * WAD as u128);

            // After interest accrual, total liquidity increases and collateral are worth more
            reserve.accrue_interest(1)?;

            let new_exchange_rate = reserve.collateral_exchange_rate()?;
            // println!("borrow_rate={},total_liquidity={},borrowed_percent={},borrowed_liquidity_wads={},owner_unclaimed_amount={},cumulative_borrow_rate_decimal={},new_exchange_rate.0={},exchange_rate.0={}",
            //     borrow_rate,total_liquidity,Rate::from_scaled_val(borrowed_percent),borrowed_liquidity_wads,owner_unclaimed, cumulative_borrow_rate_decimal,new_exchange_rate.0,exchange_rate.0);

            if borrow_rate > 0 && total_liquidity > 0 && borrowed_percent > 0 && reserve.liquidity.total_supply()?.gt(&Decimal::zero()) {
                assert!(new_exchange_rate.0 < exchange_rate.0);
            } else {
                assert_eq!(new_exchange_rate.0, exchange_rate.0);
            }
        }

        #[test]
        fn compound_interest(
            total_liquidity in u64::MAX / 6..=MAX_LIQUIDITY,
            borrowed_percent in 0..=100u8,
            optimal_utilization_rate in 0..=100u8,
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),
            slots_elapsed in 0..=SLOTS_PER_YEAR,
        ) {
              let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_percent(borrowed_percent))?;
            let mut reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    cumulative_borrow_rate_wads:Decimal::one(),
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };

            // print!("total_liquidity={},borrowed_percent={},borrowed_amount_wads={},optimal_utilization_rate={},,min_borrow_rate={},optimal_borrow_rate={},max_borrow_rate={},",
            //         total_liquidity,Rate::from_percent(borrowed_percent),borrowed_amount_wads,optimal_utilization_rate,min_borrow_rate,optimal_borrow_rate,max_borrow_rate);
            // println!("slots_elapsed={}",slots_elapsed);
            // Simulate running for max 1000 years, assuming that interest is
            // compounded at least once a year
            for i in 0..100 {
                let borrow_rate = reserve.current_borrow_rate()?;

                // reserve.liquidity.compound_interest(borrow_rate, slots_elapsed,0)?;
                if i > 90{

                    // println!("borrow_rate={}, reserve.liquidity.borrowed_amount_wads={}", borrow_rate,reserve.liquidity.borrowed_amount_wads);
                }

                // println!(" reserve.liquidity.borrowed_amount_wads={}", reserve.liquidity.borrowed_amount_wads);
                reserve.liquidity.borrowed_amount_wads.to_scaled_val()?;
            }
        }
        #[test]
        fn compound_interest_simple(
            slots_elapsed in 1..=SLOTS_PER_YEAR,
            borrow_rate in 0..=36u8,
        ) {
            let mut reserve = Pool::default();
            reserve.liquidity.borrowed_amount_wads = Decimal::from(MAX_LIQUIDITY);
            reserve.liquidity.cumulative_borrow_rate_wads = Decimal::one();
            let borrow_rate = Rate::from_percent(borrow_rate);
            // println!("slots_elapsed={},borrow_rate={}",slots_elapsed,borrow_rate);
            // Simulate running for max 1000 years, assuming that interest is
            // compounded at least once a year
            for i in 0..10 {
                reserve.liquidity.compound_interest(borrow_rate, slots_elapsed, 0)?;
                if i % 10 == 0{
                    // println!("borrowed_amount_wads={},cumulative_borrow_rate_wads={}",reserve.liquidity.borrowed_amount_wads,reserve.liquidity.cumulative_borrow_rate_wads);
                }
                reserve.liquidity.borrowed_amount_wads.to_scaled_val()?;
                reserve.liquidity.cumulative_borrow_rate_wads.to_scaled_val()?;
            }
        }

        #[test]
        fn reserve_accrue_interest(
                total_liquidity in u64::MAX / 6..=MAX_LIQUIDITY,
            borrowed_percent in 0..=100u8,
            optimal_utilization_rate in 0..=100u8,
            (min_borrow_rate, optimal_borrow_rate, max_borrow_rate) in borrow_rates(),
            slots_elapsed in 0..=SLOTS_PER_YEAR,
        ) {
            let borrowed_amount_wads = Decimal::from(total_liquidity).try_mul(Rate::from_percent(borrowed_percent))?;
            let mut reserve = Pool {
                liquidity: ReserveLiquidity {
                    borrowed_amount_wads,
                    available_amount: total_liquidity - borrowed_amount_wads.try_round_u64()?,
                    cumulative_borrow_rate_wads:Decimal::one(),
                    ..ReserveLiquidity::default()
                },
                collateral:ReserveCollateral{
                    ..ReserveCollateral::default()
                },
                config: PoolConfig { optimal_utilization_rate, min_borrow_rate, optimal_borrow_rate, max_borrow_rate, ..PoolConfig::default() },
                lottery:Ticket{
                    total_mining_speed:100,
                    kink_util_rate:50,
                    l_token_mining_index:Decimal::zero(),
                    borrow_mining_index:Decimal::zero(),
                    ..Ticket::default()
                },
                ..Pool::default()
            };

            let utilization_rate = reserve.liquidity.utilization_rate()?;
            let borrow_rate = reserve.current_borrow_rate()?;
             reserve.accrue_interest(slots_elapsed)?;
            // println!("total_liquidity={},borrowed_percent={},slots_elapsed={},utilization_rate={},optimal_utilization_rate={},min_borrow_rate={},optimal_borrow_rate={},max_borrow_rate={},borrow_rate={},borrowed_amount_wads={},reserve.liquidity.borrowed_amount_wads={}",
            //     total_liquidity,borrowed_percent,slots_elapsed,utilization_rate,optimal_utilization_rate,min_borrow_rate,optimal_borrow_rate,max_borrow_rate,borrow_rate,borrowed_amount_wads,reserve.liquidity.borrowed_amount_wads);
            if utilization_rate > Rate::zero() && slots_elapsed > 0 {
                assert!(reserve.liquidity.borrowed_amount_wads > borrowed_amount_wads);
            } else {
                assert!(reserve.liquidity.borrowed_amount_wads == borrowed_amount_wads);
            }
        }

        #[test]
        fn borrow_fee_calculation(
            borrow_fee_wad in 0..WAD, // at WAD, fee == borrow amount, which fails
            reserve_owner_fee_wad in 0..WAD,
            flash_loan_fee_wad in 0..WAD, // at WAD, fee == borrow amount, which fails
            host_fee_percentage in 0..=100u8,
            borrow_amount in 3..=u64::MAX, // start at 3 to ensure calculation success
                                           // 0, 1, and 2 are covered in the minimum tests
                                           // @FIXME: ^ no longer true
        ) {
            let fees = ReserveFees {
                borrow_fee_wad,
                reserve_owner_fee_wad,
                flash_loan_fee_wad,
                host_fee_percentage,
            };
            let (total_fee, host_fee) = fees.calculate_borrow_fees(Decimal::from(borrow_amount), FeeCalculation::Exclusive)?;

            // The total fee can't be greater than the amount borrowed, as long
            // as amount borrowed is greater than 2.
            // At a borrow amount of 2, we can get a total fee of 2 if a host
            // fee is also specified.
            assert!(total_fee <= borrow_amount);

            // the host fee can't be greater than the total fee
            assert!(host_fee <= total_fee);

            // for all fee rates greater than 0, we must have some fee
            if borrow_fee_wad > 0 {
                assert!(total_fee > 0);
            }

            if host_fee_percentage == 100 {
                // if the host fee percentage is maxed at 100%, it should get all the fee
                assert_eq!(host_fee, total_fee);
            }

            // if there's a host fee and some borrow fee, host fee must be greater than 0
            if host_fee_percentage > 0 && borrow_fee_wad > 0 {
                assert!(host_fee > 0);
            } else {
                assert_eq!(host_fee, 0);
            }
        }

        #[test]
        fn flash_loan_fee_calculation(
            borrow_fee_wad in 0..WAD, // at WAD, fee == borrow amount, which fails
            reserve_owner_fee_wad in 0..WAD,
            flash_loan_fee_wad in 0..WAD, // at WAD, fee == borrow amount, which fails
            host_fee_percentage in 0..=100u8,
            borrow_amount in 3..=u64::MAX, // start at 3 to ensure calculation success
                                           // 0, 1, and 2 are covered in the minimum tests
                                           // @FIXME: ^ no longer true
        ) {
            let fees = ReserveFees {
                borrow_fee_wad,
                reserve_owner_fee_wad,
                flash_loan_fee_wad,
                host_fee_percentage,
            };
            let (total_fee, host_fee) = fees.calculate_flash_loan_fees(Decimal::from(borrow_amount))?;

            // The total fee can't be greater than the amount borrowed, as long
            // as amount borrowed is greater than 2.
            // At a borrow amount of 2, we can get a total fee of 2 if a host
            // fee is also specified.
            assert!(total_fee <= borrow_amount);

            // the host fee can't be greater than the total fee
            assert!(host_fee <= total_fee);

            // for all fee rates greater than 0, we must have some fee
            if borrow_fee_wad > 0 {
                assert!(total_fee > 0);
            }

            if host_fee_percentage == 100 {
                // if the host fee percentage is maxed at 100%, it should get all the fee
                assert_eq!(host_fee, total_fee);
            }

            // if there's a host fee and some borrow fee, host fee must be greater than 0
            if host_fee_percentage > 0 && borrow_fee_wad > 0 {
                assert!(host_fee > 0);
            } else {
                assert_eq!(host_fee, 0);
            }
        }
    }

    #[test]
    fn borrow_fee_calculation_min_host() {
        let fees = ReserveFees {
            borrow_fee_wad: 10_000_000_000_000_000, // 1%
            reserve_owner_fee_wad: 10_000_000_000_000_000,
            flash_loan_fee_wad: 0,
            host_fee_percentage: 20,
        };

        // only 2 tokens borrowed, get error
        let err = fees
            .calculate_borrow_fees(Decimal::from(2u64), FeeCalculation::Exclusive)
            .unwrap_err();
        assert_eq!(err, PoolingError::BorrowTooSmall.into()); // minimum of 3 tokens

        // only 1 token borrowed, get error
        let err = fees
            .calculate_borrow_fees(Decimal::one(), FeeCalculation::Exclusive)
            .unwrap_err();
        assert_eq!(err, PoolingError::BorrowTooSmall.into());

        // 0 amount borrowed, 0 fee
        let (total_fee, host_fee) = fees
            .calculate_borrow_fees(Decimal::zero(), FeeCalculation::Exclusive)
            .unwrap();
        assert_eq!(total_fee, 0);
        assert_eq!(host_fee, 0);
    }

    #[test]
    fn borrow_fee_calculation_min_no_host() {
        let fees = ReserveFees {
            borrow_fee_wad: 10_000_000_000_000_000, // 1%
            reserve_owner_fee_wad: 10_000_000_000_000_000,
            flash_loan_fee_wad: 0,
            host_fee_percentage: 0,
        };

        // only 2 tokens borrowed, ok
        let (total_fee, host_fee) = fees
            .calculate_borrow_fees(Decimal::from(2u64), FeeCalculation::Exclusive)
            .unwrap();
        assert_eq!(total_fee, 1);
        assert_eq!(host_fee, 0);

        // only 1 token borrowed, get error
        let err = fees
            .calculate_borrow_fees(Decimal::one(), FeeCalculation::Exclusive)
            .unwrap_err();
        assert_eq!(err, PoolingError::BorrowTooSmall.into()); // minimum of 2 tokens

        // 0 amount borrowed, 0 fee
        let (total_fee, host_fee) = fees
            .calculate_borrow_fees(Decimal::zero(), FeeCalculation::Exclusive)
            .unwrap();
        assert_eq!(total_fee, 0);
        assert_eq!(host_fee, 0);
    }

    #[test]
    fn borrow_fee_calculation_host() {
        let fees = ReserveFees {
            borrow_fee_wad: 10_000_000_000_000_000, // 1%
            reserve_owner_fee_wad: 10_000_000_000_000_000,
            flash_loan_fee_wad: 0,
            host_fee_percentage: 20,
        };

        let (total_fee, host_fee) = fees
            .calculate_borrow_fees(Decimal::from(1000u64), FeeCalculation::Exclusive)
            .unwrap();

        assert_eq!(total_fee, 10); // 1% of 1000
        assert_eq!(host_fee, 2); // 20% of 10
    }

    #[test]
    fn borrow_fee_calculation_no_host() {
        let fees = ReserveFees {
            borrow_fee_wad: 10_000_000_000_000_000, // 1%
            reserve_owner_fee_wad: 10_000_000_000_000_000,
            flash_loan_fee_wad: 0,
            host_fee_percentage: 0,
        };

        let (total_fee, host_fee) = fees
            .calculate_borrow_fees(Decimal::from(1000u64), FeeCalculation::Exclusive)
            .unwrap();

        assert_eq!(total_fee, 10); // 1% of 1000
        assert_eq!(host_fee, 0); // 0 host fee
    }
}
