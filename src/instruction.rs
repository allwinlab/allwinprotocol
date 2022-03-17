//! Instruction types

use crate::{
    error::PoolingError,
    state::{PoolConfig},
    unpack_util::{
        unpack_u8,
        unpack_u64,
        unpack_bytes32,
        unpack_pubkey,
    },
};
use solana_program::{
    msg,
    program_error::ProgramError,
    pubkey::{Pubkey},
};
// use crate::config::ConfigType;
use crate::unpack_util::unpack_bool;

/// Instructions supported by the lending program.
#[derive(Clone, Debug, PartialEq)]
pub enum PoolingInstruction {
    // 0
    /// Initializes a new lending market.
    ///
    /// Accounts expected by this instruction:
    ///   0. `[singer]` Init lending market authority
    ///   1. `[writable]` Lending market account - uninitialized.
    ///   2. `[]` Rent sysvar.
    ///   3. `[]` Token program id.
    ///   4. `[]` Pyth oracle program id.
    InitPoolingManager {
        /// Owner authority which can add new reserves
        owner: Pubkey,
        /// Currency market prices are quoted in
        /// e.g. "USD" null padded (`*b"USD\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"`) or a SPL token mint pubkey
        quote_currency: [u8; 32],
    },

    // 1
    /// Sets the new owner of a lending market.
    ///
    /// Accounts expected by this instruction:
    ///
    ///
    ///   0. `[writable]` Lending market account.
    ///   1. `[signer]` Current owner.
    SetPoolingManagerOwner {
        /// The new owner
        new_owner: Pubkey,
    },

    // 2
    /// Initializes a new lending market reserve.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Reserve account - uninitialized.
    ///
    ///   1. `[]` Reserve liquidity SPL Token mint.
    ///   2. `[]` Reserve liquidity supply SPL Token account.
    ///   3. `[]` Reserve liquidity fee receiver.
    ///
    ///   4. `[]` Pyth product account.
    ///   5. `[]` Pyth price account.
    ///             This will be used as the reserve liquidity oracle account.
    ///   7. `[]` Reserve collateral SPL Token mint.
    ///
    ///   8. `[]` Reserve collateral token supply.
    ///   9  `[]` Lending market account.
    ///
    ///   10  `[signer]` Lending market owner.
    ///   11. `[]` Un_coll_supply_account
    ///
    ///   12  `[]` Clock sysvar.
    ///
    ///   13 `[]` Rent sysvar.
    ///   14 `[]` Token program id.

    InitPool {
        /// Reserve configuration values
        config: PoolConfig,
        total_mining_speed: u64,
        kink_util_rate: u64,
        use_pyth_oracle: bool,
    },

    // 3
    /// Accrue interest and update market price of liquidity on a reserve.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Reserve account.
    ///
    ///   1. `[]` Reserve liquidity oracle account.
    ///             Must be the Pyth price account specified at InitReserve.
    ///   3. `[]` Clock sysvar.
    RefreshPool,

    // 4
    /// Deposit liquidity into a reserve in exchange for collateral. Collateral represents a share
    /// of the reserve liquidity pool.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Source liquidity token account.
    ///                     $authority can transfer $liquidity_amount.
    ///   1. `[writable]` Destination collateral token account.
    ///   2. `[writable]` Reserve account.
    ///   3. `[writable]` Reserve collateral SPL Token mint.
    ///   4. `[writable]` Reserve liquidity supply SPL Token account.
    ///   5. `[]` Lending market account.
    ///   6. `[]` Derived lending market authority.
    ///   7. `[signer]` User transfer authority ($authority).
    ///   8. `[]` Clock sysvar.
    ///   9. `[]` Token program id.
    DepositPoolLiquidity {
        /// Amount of liquidity to deposit in exchange for collateral tokens
        liquidity_amount: u64,
    },

    // 5
    /// Redeem collateral from a reserve in exchange for liquidity.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Source collateral token account.
    ///                     $authority can transfer $collateral_amount.
    ///   1. `[writable]` Destination liquidity token account.
    ///   2. `[writable]` Reserve account.
    ///   3. `[writable]` Reserve collateral SPL Token mint.
    ///   4. `[writable]` Reserve liquidity supply SPL Token account.
    ///   5. `[]` Lending market account.
    ///   6. `[]` Derived lending market authority.
    ///   7. `[signer]` User transfer authority ($authority).
    ///   8. `[]` Clock sysvar.
    ///   9. `[]` Token program id.
    RedeemPoolCollateral {
        /// Amount of collateral tokens to redeem in exchange for liquidity
        collateral_amount: u64,
    },

    // 6
    /// Initializes a new lending market obligation.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Obligation account - uninitialized.
    ///   1. `[]` Lending market account.
    ///   2. `[signer]` Obligation owner.
    ///   3. `[]` Clock sysvar.
    ///   4. `[]` Rent sysvar.
    ///   5. `[]` Token program id.
    InitTicket,

    // 7
    /// Refresh an obligation's accrued interest and collateral and liquidity prices. Requires
    /// refreshed reserves, as all obligation collateral deposit reserves in order, followed by all
    /// liquidity borrow reserves in order.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Obligation account.
    ///   1. `[]` Clock sysvar.
    ///   .. `[]` Collateral deposit reserve accounts - refreshed, all, in order.
    ///   .. `[]` Liquidity borrow reserve accounts - refreshed, all, in order.
    RefreshTicket,

    // 8
    /// Refresh an obligation's accrued interest and collateral and liquidity prices. Requires
    /// refreshed reserves, as all obligation collateral deposit reserves in order, followed by all
    /// liquidity borrow reserves in order.
    ///
    /// Accounts expected by this instruction:
    ///
    ///   0. `[writable]` Ticket account.
    ///   1. `[]` Clock sysvar.
    LotteryDraw,
}

impl PoolingInstruction {
    /// Unpacks a byte buffer into a [LendingInstruction](enum.LendingInstruction.html).
    pub fn unpack(input: &[u8]) -> Result<Self, ProgramError> {
        let (&tag, rest) = input
            .split_first()
            .ok_or(PoolingError::InstructionUnpackError)?;
        Ok(match tag {
            0 => {
                let (owner, rest) = unpack_pubkey(rest)?;
                let (quote_currency, _rest) = unpack_bytes32(rest)?;

                Self::InitPoolingManager {
                    owner,
                    quote_currency: *quote_currency,
                }
            }
            1 => {
                let (new_owner, _rest) = unpack_pubkey(rest)?;
                Self::SetPoolingManagerOwner { new_owner }
            }
            2 => {
                let (total_mining_speed, rest) = unpack_u64(rest)?;
                let (kink_util_rate, rest) = unpack_u64(rest)?;
                let (use_pyth_oracle, _rest) = unpack_bool(rest)?;
                Self::InitPool {
                    config: PoolConfig {
                        deposit_paused: false,
                    },
                    total_mining_speed,
                    kink_util_rate,
                    use_pyth_oracle,
                }
            }
            3 => Self::RefreshPool,
            4 => {
                let (liquidity_amount, _rest) = unpack_u64(rest)?;
                Self::DepositPoolLiquidity { liquidity_amount }
            }
            5 => {
                let (collateral_amount, _rest) = unpack_u64(rest)?;
                Self::RedeemPoolCollateral { collateral_amount }
            }
            6 => Self::InitTicket,
            7 => Self::RefreshTicket,
            8 => {
                Self::LotteryDraw
            }
            _ => {
                msg!("Instruction cannot be unpacked");
                return Err(PoolingError::InstructionUnpackError.into());
            }
        })
    }
}

