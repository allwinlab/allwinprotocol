//! Program state processor

use std::convert::TryInto;

use num_traits::FromPrimitive;
use solana_program::{
    account_info::{AccountInfo, next_account_info},
    decode_error::DecodeError,
    entrypoint::ProgramResult,
    instruction::Instruction,
    msg,
    program::{invoke, invoke_signed},
    program_error::{PrintProgramError, ProgramError},
    program_pack::{IsInitialized, Pack},
    pubkey::Pubkey,
    sysvar::{clock::Clock, rent::Rent, Sysvar},
};
use spl_token::solana_program::instruction::AccountMeta;
use spl_token::state::{Account, Mint};

use crate::{
    error::PoolingError,
    instruction::PoolingInstruction,
    math::{Decimal, Rate, TryAdd, TryDiv, TryMul},
    pyth,
    state::{
        CalculateBorrowResult, CalculateLiquidationResult, CalculateRepayResult,
        InitPoolManagerParams, InitTicketParams, InitPoolParams, PoolManager,
        NewReserveCollateralParams, NewReserveLiquidityParams, Ticket, Pool,
        ReserveCollateral, PoolConfig, ReserveLiquidity,
    },
};
use crate::math::{TrySub, WAD};
use crate::state::{Lottery, init_pool_accounts_index, InitBonusParams, InitMiningParams, Mining};


/// Processes an instruction
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    input: &[u8],
) -> ProgramResult {
    let instruction = PoolingInstruction::unpack(input)?;
    match instruction {
        PoolingInstruction::InitPoolingManager {
            owner,
            quote_currency,
        } => {
            msg!("Instruction: Init Pool Manager");
            process_init_pool_manager(program_id, owner, quote_currency, accounts)
        }
        PoolingInstruction::InitPool {
            config,
            total_mining_speed,
            kink_util_rate,
            use_pyth_oracle
        } => {
            msg!("Instruction: Init Pool");
            process_init_pool(program_id, config, total_mining_speed, kink_util_rate, use_pyth_oracle, accounts)
        }
        PoolingInstruction::InitTicket => {
            msg!("Instruction: Init Ticket");
            process_init_ticket(program_id, accounts)
        }
        PoolingInstruction::DepositPoolLiquidity { liquidity_amount } => {
            msg!("Instruction: Deposit Reserve Liquidity into pool");
            process_deposit_pool_liquidity(program_id, liquidity_amount, accounts)
        }
        PoolingInstruction::RedeemPoolCollateral { collateral_amount } => {
            msg!("Instruction: Redeem Reserve Collateral out of pool");
            process_redeem_pool_collateral(program_id, collateral_amount, accounts)
        }
        PoolingInstruction::RefreshPool => {
            msg!("Instruction: Refresh Reserve");
            process_refresh_reserve(program_id, accounts)
        }
        // PoolingInstruction::DepositObligationCollateral { collateral_amount } => {
        //     msg!("Instruction: Deposit Obligation Collateral");
        //     process_deposit_obligation_collateral(program_id, collateral_amount, accounts)
        // }
        // PoolingInstruction::WithdrawObligationCollateral { collateral_amount } => {
        //     msg!("Instruction: Withdraw Obligation Collateral");
        //     process_withdraw_obligation_collateral(program_id, collateral_amount, accounts)
        // }
        PoolingInstruction::SetPoolingManagerOwner { new_owner } => {
            msg!("Instruction: Set Pool Manager Owner");
            process_set_pool_manager_owner(program_id, new_owner, accounts)
        }
        PoolingInstruction::RefreshTicket => {
            msg!("Instruction: Refresh Ticket");
            process_refresh_ticket(program_id, accounts)
        }
        PoolingInstruction::LotteryDraw => {
            msg!("Instruction: Set Pool Manager Owner");
            process_lottery_draw(program_id, accounts)
        }
    }
}

fn process_init_pool_manager(
    program_id: &Pubkey,
    owner: Pubkey,
    quote_currency: [u8; 32],
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let init_pool_manager_authority_info = next_account_info(account_info_iter)?;
    let pool_manager_info = next_account_info(account_info_iter)?;
    let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;
    let token_program_id = next_account_info(account_info_iter)?;
    let pyth_oracle_program_id = next_account_info(account_info_iter)?;
    let mine_account_info = next_account_info(account_info_iter)?;
    let mine_supply_account_info = next_account_info(account_info_iter)?;
    // for open source, this restrict can be lifted
    if init_pool_manager_authority_info.key.to_string() != "7NzERexiPdyiNp5whD74AwTDpALp5VgPta6hmdcGuNm9" {
        msg!("Can not init pool manager");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if !init_pool_manager_authority_info.is_signer {
        msg!("Init pool manager authority account must be a signer");
        return Err(PoolingError::InvalidSigner.into());
    }
    assert_rent_exempt(rent, pool_manager_info)?;
    let mut pool_manager = assert_uninitialized::<PoolManager>(pool_manager_info)?;
    if pool_manager_info.owner != program_id {
        msg!("Pool manager provided is not owned by the pool program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    pool_manager.init(InitPoolManagerParams {
        bump_seed: Pubkey::find_program_address(&[pool_manager_info.key.as_ref()], program_id).1,
        owner,
        quote_currency,
        token_program_id: *token_program_id.key,
        oracle_program_id: *pyth_oracle_program_id.key,
        mine_mint: *mine_account_info.key,
        mine_supply_account: *mine_supply_account_info.key,
    });
    PoolManager::pack(pool_manager, &mut pool_manager_info.data.borrow_mut())?;
    Ok(())
}

#[inline(never)] // avoid stack frame limit
fn process_set_pool_manager_owner(
    _program_id: &Pubkey,
    _new_owner: Pubkey,
    _accounts: &[AccountInfo],
) -> ProgramResult {
    msg!("Abandoned method ");
    Ok(())
}

#[inline(never)] // avoid stack frame limit
fn process_init_pool(
    program_id: &Pubkey,
    config: PoolConfig,
    total_mining_speed: u64,
    kink_util_rate: u64,
    use_pyth_oracle: bool,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let clock = &Clock::from_account_info(accounts.get(init_pool_accounts_index::CLOCK_SYSVAR).ok_or(PoolingError::InvalidAccountInput)?)?;
    let rent = &Rent::from_account_info(accounts.get(init_pool_accounts_index::RENT_SYSVAR).ok_or(PoolingError::InvalidAccountInput)?)?;
    assert_rent_exempt(rent, accounts.get(init_pool_accounts_index::RESERVE_ACCOUNT)
        .ok_or(PoolingError::InvalidAccountInput)?)?;
    if accounts.get(init_pool_accounts_index::RESERVE_ACCOUNT)
        .ok_or(PoolingError::InvalidAccountInput)?.owner
        !=
        program_id {
        msg!("Reserve provided is not owned by the pooling program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if accounts.get(init_pool_accounts_index::LIQUIDITY_FEE_RECEIVER)
        .ok_or(PoolingError::InvalidAccountInput)?.owner
        !=
        &spl_token::id() {
        msg!("Reserve liquidity fee receiver is not owned by spl-token program");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    let liquidity_fee_receiver = Account::unpack(
        &accounts.get(init_pool_accounts_index::LIQUIDITY_FEE_RECEIVER)
            .ok_or(PoolingError::InvalidAccountInput)?.data.borrow()
    )?;
    if liquidity_fee_receiver.mint != *accounts.get(init_pool_accounts_index::LIQUIDITY_MINT).ok_or(PoolingError::InvalidAccountInput)?.key {
        msg!("Reserve liquidity fee receiver is not a token account of reserve liquidity");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    let pool_manager = PoolManager::unpack(&accounts.get(init_pool_accounts_index::POOL_MANAGER).ok_or(PoolingError::InvalidAccountInput)?.data.borrow())?;
    if &pool_manager.owner != accounts.get(init_pool_accounts_index::POOL_MANAGER_OWNER).ok_or(PoolingError::InvalidAccountInput)?.key {
        msg!("Pool manager owner does not match the pool manager owner provided");
        return Err(PoolingError::InvalidMarketOwner.into());
    }
    if !accounts.get(init_pool_accounts_index::POOL_MANAGER_OWNER).ok_or(PoolingError::InvalidAccountInput)?.is_signer {
        msg!("Lending market owner provided must be a signer");
        return Err(PoolingError::InvalidSigner.into());
    }
    if accounts.get(init_pool_accounts_index::POOL_MANAGER).ok_or(PoolingError::InvalidAccountInput)?.owner != program_id {
        msg!("Lending market provided is not owned by the lending program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &pool_manager.token_program_id != accounts.get(init_pool_accounts_index::TOKEN_PROGRAM_ID).ok_or(PoolingError::InvalidAccountInput)?.key {
        msg!("Lending market token program does not match the token program provided");
        return Err(PoolingError::InvalidTokenProgram.into());
    }
    if &pool_manager.oracle_program_id != accounts.get(init_pool_accounts_index::PYTH_PRODUCT).ok_or(PoolingError::InvalidAccountInput)?.owner {
        msg!("Pyth product account provided is not owned by the lending market oracle program");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    if &pool_manager.oracle_program_id != accounts.get(init_pool_accounts_index::PYTH_PRICE).ok_or(PoolingError::InvalidAccountInput)?.owner {
        msg!("Pyth price account provided is not owned by the lending market oracle program");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    let pyth_product_data = accounts.get(init_pool_accounts_index::PYTH_PRODUCT).ok_or(PoolingError::InvalidAccountInput)?.try_borrow_data()?;
    let pyth_product = pyth::load::<pyth::Product>(&pyth_product_data)
        .map_err(|_| ProgramError::InvalidAccountData)?;
    if pyth_product.magic != pyth::MAGIC {
        msg!("Pyth product account provided is not a valid Pyth account");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    if pyth_product.ver != pyth::VERSION_2 {
        msg!("Pyth product account provided has a different version than expected");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    if pyth_product.atype != pyth::AccountType::Product as u32 {
        msg!("Pyth product account provided is not a valid Pyth product account");
        return Err(PoolingError::InvalidOracleConfig.into());
    }

    let pyth_price_pubkey_bytes: &[u8; 32] = accounts.get(init_pool_accounts_index::PYTH_PRICE).ok_or(PoolingError::InvalidAccountInput)?
        .key
        .as_ref()
        .try_into()
        .map_err(|_| PoolingError::InvalidAccountInput)?;
    if &pyth_product.px_acc.val != pyth_price_pubkey_bytes {
        msg!("Pyth product price account does not match the Pyth price provided");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    let quote_currency = get_pyth_product_quote_currency(pyth_product)?;
    if pool_manager.quote_currency != quote_currency {
        msg!("Lending market quote currency does not match the oracle quote currency");
        return Err(PoolingError::InvalidOracleConfig.into());
    }
    if accounts.get(init_pool_accounts_index::LIQUIDITY_MINT)
        .ok_or(PoolingError::InvalidAccountInput)?.owner != accounts.get(init_pool_accounts_index::TOKEN_PROGRAM_ID).ok_or(PoolingError::InvalidAccountInput)?.key {
        msg!("Reserve liquidity mint is not owned by the token program provided");
        return Err(PoolingError::InvalidTokenOwner.into());
    }
    let market_price = get_pyth_price(accounts.get(init_pool_accounts_index::PYTH_PRICE).ok_or(PoolingError::InvalidAccountInput)?, clock)?;
    msg!(&market_price.to_string());
    let reserve_liquidity_mint = unpack_mint(&accounts.get(init_pool_accounts_index::LIQUIDITY_MINT)
        .ok_or(PoolingError::InvalidAccountInput)?.data.borrow())?;
    let clock = &Clock::from_account_info(accounts.get(init_pool_accounts_index::CLOCK_SYSVAR).ok_or(PoolingError::InvalidAccountInput)?)?;
    let mut reserve = assert_uninitialized::<Pool>(accounts.get(init_pool_accounts_index::RESERVE_ACCOUNT).ok_or(PoolingError::InvalidAccountInput)?)?;
    reserve.init(InitPoolParams {
        current_slot: clock.slot,
        pool_manager: *accounts.get(init_pool_accounts_index::POOL_MANAGER).ok_or(PoolingError::InvalidAccountInput)?.key,
        liquidity: ReserveLiquidity::new(NewReserveLiquidityParams {
            mint_pubkey: *accounts.get(init_pool_accounts_index::LIQUIDITY_MINT).ok_or(PoolingError::InvalidAccountInput)?.key,
            mint_decimals: reserve_liquidity_mint.decimals,
            supply_pubkey: *accounts.get(init_pool_accounts_index::LIQUIDITY_SUPPLY).ok_or(PoolingError::InvalidAccountInput)?.key,
            fee_receiver: *accounts.get(init_pool_accounts_index::LIQUIDITY_FEE_RECEIVER).ok_or(PoolingError::InvalidAccountInput)?.key,
            use_pyth_oracle,
            pyth_oracle_pubkey: *accounts.get(init_pool_accounts_index::PYTH_PRICE).ok_or(PoolingError::InvalidAccountInput)?.key,
            market_price,
        }),
        collateral: ReserveCollateral::new(NewReserveCollateralParams {
            mint_pubkey: *accounts.get(init_pool_accounts_index::COLLATERAL_MINT).ok_or(PoolingError::InvalidAccountInput)?.key,
            supply_pubkey: *accounts.get(init_pool_accounts_index::COLLATERAL_SUPPLY).ok_or(PoolingError::InvalidAccountInput)?.key,
        }),
        lottery: Lottery::new(InitBonusParams {
            un_coll_supply_account: *accounts.get(init_pool_accounts_index::UN_COLL_SUPPLY).ok_or(PoolingError::InvalidAccountInput)?.key,
            total_mining_speed,
            kink_util_rate,
        }),
        config,
    });
    Pool::pack(reserve, &mut accounts.get(init_pool_accounts_index::RESERVE_ACCOUNT).ok_or(PoolingError::InvalidAccountInput)?.data.borrow_mut())?;
    Ok(())
}

#[inline(never)] // avoid stack frame limit
fn process_init_ticket(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let ticket_info = next_account_info(account_info_iter)?;
    let pool_manager_info = next_account_info(account_info_iter)?;
    let ticket_owner_info = next_account_info(account_info_iter)?;

    let clock = &Clock::from_account_info(next_account_info(account_info_iter)?)?;
    let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;
    let token_program_id = next_account_info(account_info_iter)?;

    assert_rent_exempt(rent, ticket_info)?;
    let mut ticket = assert_uninitialized::<Ticket>(ticket_info)?;
    if ticket_info.owner != program_id {
        msg!("Obligation provided is not owned by the pool manager");
        return Err(PoolingError::InvalidAccountOwner.into());
    }

    let pool_manager = PoolManager::unpack(&pool_manager_info.data.borrow())?;
    if pool_manager_info.owner != program_id {
        msg!("Pool manager provided is not owned by the pooling program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &pool_manager.token_program_id != token_program_id.key {
        msg!("Pool manager token program does not match the token program provided");
        return Err(PoolingError::InvalidTokenProgram.into());
    }

    if !ticket_owner_info.is_signer {
        msg!("Obligation owner provided must be a signer");
        return Err(PoolingError::InvalidSigner.into());
    }

    ticket.init(InitTicketParams {
        current_slot: clock.slot,
        pool_manager: *pool_manager_info.key,
        owner: *ticket_owner_info.key,
        deposits: vec![],
    });
    Ticket::pack(ticket, &mut ticket_info.data.borrow_mut())?;

    Ok(())
}

fn process_refresh_reserve(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let account_info_iter = &mut accounts.iter().peekable();
    let reserve_info = next_account_info(account_info_iter)?;
    let reserve_liquidity_oracle_info = next_account_info(account_info_iter)?;
    let clock = &Clock::from_account_info(next_account_info(account_info_iter)?)?;
    let mut reserve = Pool::unpack(&reserve_info.data.borrow())?;
    if reserve_info.owner != program_id {
        msg!("Reserve provided is not owned by the lending program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    msg!("reserve.liquidity.use_pyth_oracle={}",reserve.liquidity.use_pyth_oracle.to_string());

    if &reserve.liquidity.pyth_oracle_pubkey != reserve_liquidity_oracle_info.key {
        msg!("Reserve liquidity oracle does not match the reserve liquidity oracle provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    reserve.liquidity.market_price = get_pyth_price(reserve_liquidity_oracle_info, clock)?;
    msg!("reserve.liquidity.market_price={}",reserve.liquidity.market_price.to_string());
    reserve.refresh_index(clock.slot)?;
    reserve.last_update.update_slot(clock.slot);
    Pool::pack(reserve, &mut reserve_info.data.borrow_mut())?;
    Ok(())
}

fn process_refresh_ticket(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let account_info_iter = &mut accounts.iter().peekable();
    let ticket_info = next_account_info(account_info_iter)?;
    let clock = &Clock::from_account_info(next_account_info(account_info_iter)?)?;
    let mut ticket = Ticket::unpack(&ticket_info.data.borrow())?;
    if ticket_info.owner != program_id {
        msg!("Ticket provided is not owned by the pooling program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    let mut deposited_value = Decimal::zero();
    for pos in 0..ticket.deposits.len() {
        let deposit_reserve_info = next_account_info(account_info_iter)?;
        if deposit_reserve_info.owner != program_id {
            msg!(
                "Deposit reserve provided for collateral {} is not owned by the pooling program",
                pos
            );
            return Err(PoolingError::InvalidAccountOwner.into());
        }
        if &ticket.deposits[pos].deposit_reserve != deposit_reserve_info.key {
            msg!(
                "Deposit reserve of collateral {} does not match the deposit reserve provided",
                pos
            );
            return Err(PoolingError::InvalidAccountInput.into());
        }
        let deposit_reserve = Pool::unpack(&deposit_reserve_info.data.borrow())?;
        if deposit_reserve.last_update.is_stale(clock.slot)? {
            msg!(
                "Deposit reserve provided for collateral {} is stale and must be refreshed in the current slot",
                pos
            );
            return Err(PoolingError::ReserveStale.into());
        }
        // @TODO: add lookup table https://git.io/JOCYq
        let decimals = 10u64
            .checked_pow(deposit_reserve.liquidity.mint_decimals as u32)
            .ok_or(PoolingError::MathOverflow)?;
        let market_value = deposit_reserve
            .collateral_exchange_rate()?
            .decimal_collateral_to_liquidity(ticket.deposits[pos].deposited_amount.into())?
            .try_mul(deposit_reserve.liquidity.market_price)?
            .try_div(decimals)?;
        ticket.deposits[pos].market_value = market_value;
        deposited_value = deposited_value.try_add(market_value)?;
        ticket.refresh_deposit_unclaimed(pos, &deposit_reserve)?;
    }
    if account_info_iter.peek().is_some() {
        msg!("Too many deposit reserves provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    ticket.deposited_value = deposited_value;
    ticket.last_update.update_slot(clock.slot);
    Ticket::pack(ticket, &mut ticket_info.data.borrow_mut())?;
    Ok(())
}

fn process_deposit_pool_liquidity(
    program_id: &Pubkey,
    amount: u64,
    accounts: &[AccountInfo],
) -> ProgramResult {
    if amount == 0 {
        msg!("Liquidity amount provided cannot be zero");
        return Err(PoolingError::InvalidAmount.into());
    }
    let account_info_iter = &mut accounts.iter();

    let source_liquidity_info = next_account_info(account_info_iter)?;
    let destination_collateral_info = next_account_info(account_info_iter)?;
    let reserve_info = next_account_info(account_info_iter)?;

    let reserve_collateral_mint_info = next_account_info(account_info_iter)?;
    let reserve_liquidity_supply_info = next_account_info(account_info_iter)?;
    let pool_manager_info = next_account_info(account_info_iter)?;

    let pool_manager_authority_info = next_account_info(account_info_iter)?;
    let user_transfer_authority_info = next_account_info(account_info_iter)?;
    let clock = &Clock::from_account_info(next_account_info(account_info_iter)?)?;

    let token_program_id = next_account_info(account_info_iter)?;

    let pool_manager = PoolManager::unpack(&pool_manager_info.data.borrow())?;
    if pool_manager_info.owner != program_id {
        msg!("Pool manager provided is not owned by the pooling program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &pool_manager.token_program_id != token_program_id.key {
        msg!("Pool manager token program does not match the token program provided");
        return Err(PoolingError::InvalidTokenProgram.into());
    }
    let mut reserve = Pool::unpack(&reserve_info.data.borrow())?;
    if reserve_info.owner != program_id {
        msg!("Reserve provided is not owned by the pooling program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &reserve.pool_manager != pool_manager_info.key {
        msg!("pool's manager does not match the pool manager provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.liquidity.supply_pubkey != reserve_liquidity_supply_info.key {
        msg!("Reserve liquidity supply does not match the reserve liquidity supply provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.collateral.mint_pubkey != reserve_collateral_mint_info.key {
        msg!("Reserve collateral mint does not match the reserve collateral mint provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.liquidity.supply_pubkey == source_liquidity_info.key {
        msg!("Reserve liquidity supply cannot be used as the source liquidity provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }

    if &reserve.collateral.supply_pubkey == destination_collateral_info.key {
        msg!("Reserve collateral supply cannot be used as the destination collateral provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if reserve.last_update.is_stale(clock.slot)? {
        msg!("Reserve is stale and must be refreshed in the current slot");
        return Err(PoolingError::ReserveStale.into());
    }

    if reserve.reentry_lock {
        msg!("Can not reentry");
        return Err(PoolingError::ReentryLocked.into());
    }
    if reserve.config.deposit_paused {
        msg!("Deposits to this reserve is paused");
        return Err(PoolingError::DepositPaused.into());
    }
    let authority_signer_seeds = &[
        pool_manager_info.key.as_ref(),
        &[pool_manager.bump_seed],
    ];
    let pool_manager_authority_pubkey =
        Pubkey::create_program_address(authority_signer_seeds, program_id)?;
    if &pool_manager_authority_pubkey != pool_manager_authority_info.key {
        msg!(
            "Derived pool manager authority does not match the pool manager authority provided"
        );
        return Err(PoolingError::InvalidMarketAuthority.into());
    }

    let liquidity_account = Account::unpack(&source_liquidity_info.data.borrow())?;
    let destination_collateral_account = Account::unpack(&destination_collateral_info.data.borrow())?;
    if destination_collateral_account.owner != liquidity_account.owner {
        msg!("Destination collateral account owner must match liquidity account owner");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    let liquidity_amount = if amount == u64::MAX {
        liquidity_account.amount
    } else {
        if amount > liquidity_account.amount {
            msg!("Deposit amount too large for account balance");
            return Err(PoolingError::DepositAmountTooLarge.into());
        };
        amount
    };
    let collateral_amount = reserve.deposit_liquidity(liquidity_amount)?;
    reserve.last_update.mark_stale();
    Pool::pack(reserve, &mut reserve_info.data.borrow_mut())?;
    spl_token_transfer(TokenTransferParams {
        source: source_liquidity_info.clone(),
        destination: reserve_liquidity_supply_info.clone(),
        amount: liquidity_amount,
        authority: user_transfer_authority_info.clone(),
        authority_signer_seeds: &[],
        token_program: token_program_id.clone(),
    })?;

    spl_token_mint_to(TokenMintToParams {
        mint: reserve_collateral_mint_info.clone(),
        destination: destination_collateral_info.clone(),
        amount: collateral_amount,
        authority: pool_manager_authority_info.clone(),
        authority_signer_seeds,
        token_program: token_program_id.clone(),
    })?;
    Ok(())
}

fn process_redeem_pool_collateral(
    program_id: &Pubkey,
    amount: u64,
    accounts: &[AccountInfo],
) -> ProgramResult {
    if amount == 0 {
        msg!("Collateral amount provided cannot be zero");
        return Err(PoolingError::InvalidAmount.into());
    }

    let account_info_iter = &mut accounts.iter();

    let source_collateral_info = next_account_info(account_info_iter)?;
    let destination_liquidity_info = next_account_info(account_info_iter)?;
    let reserve_info = next_account_info(account_info_iter)?;

    let reserve_collateral_mint_info = next_account_info(account_info_iter)?;
    let reserve_liquidity_supply_info = next_account_info(account_info_iter)?;
    let lending_market_info = next_account_info(account_info_iter)?;

    let lending_market_authority_info = next_account_info(account_info_iter)?;
    let user_transfer_authority_info = next_account_info(account_info_iter)?;
    let clock = &Clock::from_account_info(next_account_info(account_info_iter)?)?;

    let token_program_id = next_account_info(account_info_iter)?;


    let lending_market = PoolManager::unpack(&lending_market_info.data.borrow())?;
    if lending_market_info.owner != program_id {
        msg!("Lending market provided is not owned by the lending program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &lending_market.token_program_id != token_program_id.key {
        msg!("Lending market token program does not match the token program provided");
        return Err(PoolingError::InvalidTokenProgram.into());
    }

    let mut reserve = Pool::unpack(&reserve_info.data.borrow())?;
    if reserve_info.owner != program_id {
        msg!("Reserve provided is not owned by the lending program");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    if &reserve.pool_manager != lending_market_info.key {
        msg!("Reserve lending market does not match the lending market provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.collateral.mint_pubkey != reserve_collateral_mint_info.key {
        msg!("Reserve collateral mint does not match the reserve collateral mint provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.collateral.supply_pubkey == source_collateral_info.key {
        msg!("Reserve collateral supply cannot be used as the source collateral provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.liquidity.supply_pubkey != reserve_liquidity_supply_info.key {
        msg!("Reserve liquidity supply does not match the reserve liquidity supply provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if &reserve.liquidity.supply_pubkey == destination_liquidity_info.key {
        msg!("Reserve liquidity supply cannot be used as the destination liquidity provided");
        return Err(PoolingError::InvalidAccountInput.into());
    }
    if reserve.last_update.is_stale(clock.slot)? {
        msg!("Reserve is stale and must be refreshed in the current slot");
        return Err(PoolingError::ReserveStale.into());
    }
    if reserve.reentry_lock {
        msg!("Can not reentry");
        return Err(PoolingError::ReentryLocked.into());
    }
    let authority_signer_seeds = &[
        lending_market_info.key.as_ref(),
        &[lending_market.bump_seed],
    ];
    let lending_market_authority_pubkey =
        Pubkey::create_program_address(authority_signer_seeds, program_id)?;
    if &lending_market_authority_pubkey != lending_market_authority_info.key {
        msg!(
            "Derived lending market authority does not match the lending market authority provided"
        );
        return Err(PoolingError::InvalidMarketAuthority.into());
    }
    let collateral_account = Account::unpack(&source_collateral_info.data.borrow())?;
    let destination_liquidity_account = Account::unpack(&destination_liquidity_info.data.borrow())?;
    if destination_liquidity_account.owner != collateral_account.owner {
        msg!("Destination liquidity account owner must match collateral account owner");
        return Err(PoolingError::InvalidAccountOwner.into());
    }
    let collateral_amount = if amount == u64::MAX {
        collateral_account.amount
    } else {
        if amount > collateral_account.amount {
            msg!("Redeem amount too large for account balance");
            return Err(PoolingError::RedeemAmountTooLarge.into());
        };
        amount
    };
    let liquidity_amount = reserve.redeem_collateral(collateral_amount)?;
    reserve.last_update.mark_stale();
    Pool::pack(reserve, &mut reserve_info.data.borrow_mut())?;

    spl_token_burn(TokenBurnParams {
        mint: reserve_collateral_mint_info.clone(),
        source: source_collateral_info.clone(),
        amount: collateral_amount,
        authority: user_transfer_authority_info.clone(),
        authority_signer_seeds: &[],
        token_program: token_program_id.clone(),
    })?;

    spl_token_transfer(TokenTransferParams {
        source: reserve_liquidity_supply_info.clone(),
        destination: destination_liquidity_info.clone(),
        amount: liquidity_amount,
        authority: lending_market_authority_info.clone(),
        authority_signer_seeds,
        token_program: token_program_id.clone(),
    })?;

    Ok(())
}

fn process_lottery_draw(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    Ok(())
}

fn assert_rent_exempt(rent: &Rent, account_info: &AccountInfo) -> ProgramResult {
    if !rent.is_exempt(account_info.lamports(), account_info.data_len()) {
        msg!(&rent.minimum_balance(account_info.data_len()).to_string());
        Err(PoolingError::NotRentExempt.into())
    } else {
        Ok(())
    }
}

fn assert_uninitialized<T: Pack + IsInitialized>(
    account_info: &AccountInfo,
) -> Result<T, ProgramError> {
    let account: T = T::unpack_unchecked(&account_info.data.borrow())?;
    if account.is_initialized() {
        Err(PoolingError::AlreadyInitialized.into())
    } else {
        Ok(account)
    }
}

/// Unpacks a spl_token `Mint`.
fn unpack_mint(data: &[u8]) -> Result<Mint, PoolingError> {
    Mint::unpack(data).map_err(|_| PoolingError::InvalidTokenMint)
}


fn get_pyth_price(pyth_price_info: &AccountInfo, _clock: &Clock) -> Result<Decimal, ProgramError> {
    // const STALE_AFTER_SLOTS_ELAPSED: u64 = 5;

    let pyth_price_data = pyth_price_info.try_borrow_data()?;
    let pyth_price = pyth::load::<pyth::Price>(&pyth_price_data)
        .map_err(|_| ProgramError::InvalidAccountData)?;

    if pyth_price.ptype != pyth::PriceType::Price {
        msg!("Oracle price type is invalid");
        return Err(PoolingError::InvalidOracleConfig.into());
    }


    let price: u64 = pyth_price.agg.price.try_into().map_err(|_| {
        msg!("Oracle price cannot be negative");
        PoolingError::InvalidOracleConfig
    })?;

    let market_price = if pyth_price.expo >= 0 {
        let exponent = pyth_price
            .expo
            .try_into()
            .map_err(|_| PoolingError::MathOverflow)?;
        let zeros = 10u64
            .checked_pow(exponent)
            .ok_or(PoolingError::MathOverflow)?;
        Decimal::from(price).try_mul(zeros)?
    } else {
        let exponent = pyth_price
            .expo
            .checked_abs()
            .ok_or(PoolingError::MathOverflow)?
            .try_into()
            .map_err(|_| PoolingError::MathOverflow)?;
        let decimals = 10u64
            .checked_pow(exponent)
            .ok_or(PoolingError::MathOverflow)?;
        Decimal::from(price).try_div(decimals)?
    };

    Ok(market_price)
}

#[inline(always)]
fn invoke_optionally_signed(
    instruction: &Instruction,
    account_infos: &[AccountInfo],
    authority_signer_seeds: &[&[u8]],
) -> ProgramResult {
    if authority_signer_seeds.is_empty() {
        invoke(instruction, account_infos)
    } else {
        invoke_signed(instruction, account_infos, &[authority_signer_seeds])
    }
}

/// Issue a spl_token `Transfer` instruction.
#[inline(always)]
fn spl_token_transfer(params: TokenTransferParams<'_, '_>) -> ProgramResult {
    let TokenTransferParams {
        source,
        destination,
        authority,
        token_program,
        amount,
        authority_signer_seeds,
    } = params;
    let result = invoke_optionally_signed(
        &spl_token::instruction::transfer(
            token_program.key,
            source.key,
            destination.key,
            authority.key,
            &[],
            amount,
        )?,
        &[source, destination, authority, token_program],
        authority_signer_seeds,
    );
    result.map_err(|_| PoolingError::TokenTransferFailed.into())
}

/// Issue a spl_token `MintTo` instruction.
fn spl_token_mint_to(params: TokenMintToParams<'_, '_>) -> ProgramResult {
    let TokenMintToParams {
        mint,
        destination,
        authority,
        token_program,
        amount,
        authority_signer_seeds,
    } = params;
    let result = invoke_optionally_signed(
        &spl_token::instruction::mint_to(
            token_program.key,
            mint.key,
            destination.key,
            authority.key,
            &[],
            amount,
        )?,
        &[mint, destination, authority, token_program],
        authority_signer_seeds,
    );
    result.map_err(|_| PoolingError::TokenMintToFailed.into())
}

/// Issue a spl_token `Burn` instruction.
#[inline(always)]
fn spl_token_burn(params: TokenBurnParams<'_, '_>) -> ProgramResult {
    let TokenBurnParams {
        mint,
        source,
        authority,
        token_program,
        amount,
        authority_signer_seeds,
    } = params;
    let result = invoke_optionally_signed(
        &spl_token::instruction::burn(
            token_program.key,
            source.key,
            mint.key,
            authority.key,
            &[],
            amount,
        )?,
        &[source, mint, authority, token_program],
        authority_signer_seeds,
    );
    result.map_err(|_| PoolingError::TokenBurnFailed.into())
}

// struct TokenInitializeMintParams<'a: 'b, 'b> {
//     mint: AccountInfo<'a>,
//     rent: AccountInfo<'a>,
//     authority: &'b Pubkey,
//     decimals: u8,
//     token_program: AccountInfo<'a>,
// }
//
// struct TokenInitializeAccountParams<'a> {
//     account: AccountInfo<'a>,
//     mint: AccountInfo<'a>,
//     owner: AccountInfo<'a>,
//     rent: AccountInfo<'a>,
//     token_program: AccountInfo<'a>,
// }

struct TokenTransferParams<'a: 'b, 'b> {
    source: AccountInfo<'a>,
    destination: AccountInfo<'a>,
    amount: u64,
    authority: AccountInfo<'a>,
    authority_signer_seeds: &'b [&'b [u8]],
    token_program: AccountInfo<'a>,
}

struct TokenMintToParams<'a: 'b, 'b> {
    mint: AccountInfo<'a>,
    destination: AccountInfo<'a>,
    amount: u64,
    authority: AccountInfo<'a>,
    authority_signer_seeds: &'b [&'b [u8]],
    token_program: AccountInfo<'a>,
}

struct TokenBurnParams<'a: 'b, 'b> {
    mint: AccountInfo<'a>,
    source: AccountInfo<'a>,
    amount: u64,
    authority: AccountInfo<'a>,
    authority_signer_seeds: &'b [&'b [u8]],
    token_program: AccountInfo<'a>,
}

impl PrintProgramError for PoolingError {
    fn print<E>(&self)
        where
            E: 'static + std::error::Error + DecodeError<E> + PrintProgramError + FromPrimitive,
    {
        msg!(&self.to_string());
    }
}


pub fn get_pyth_product_quote_currency(pyth_product: &pyth::Product) -> Result<[u8; 32], ProgramError> {
    const LEN: usize = 14;
    const KEY: &[u8; LEN] = b"quote_currency";

    let mut start = 0;
    while start < pyth::PROD_ATTR_SIZE {
        let mut length = pyth_product.attr[start] as usize;
        start += 1;

        if length == LEN {
            let mut end = start + length;
            if end > pyth::PROD_ATTR_SIZE {
                msg!("Pyth product attribute key length too long");
                return Err(PoolingError::InvalidOracleConfig.into());
            }

            let key = &pyth_product.attr[start..end];
            if key == KEY {
                start += length;
                length = pyth_product.attr[start] as usize;
                start += 1;

                end = start + length;
                if length > 32 || end > pyth::PROD_ATTR_SIZE {
                    msg!("Pyth product quote currency value too long");
                    return Err(PoolingError::InvalidOracleConfig.into());
                }

                let mut value = [0u8; 32];
                value[0..length].copy_from_slice(&pyth_product.attr[start..end]);
                return Ok(value);
            }
        }

        start += length;
        start += 1 + pyth_product.attr[start] as usize;
    }

    msg!("Pyth product quote currency not found");
    Err(PoolingError::InvalidOracleConfig.into())
}

