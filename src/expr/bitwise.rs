use super::{Expression, Rule, ToSlice};
use crate::sys::{self, libc};
use std::ffi::c_void;
use std::os::raw::c_char;

/// Expression for performing bitwise masking and XOR on the data in a register.
pub struct Bitwise<M: ToSlice, X: ToSlice> {
    mask: M,
    xor: X,
}

impl<M: ToSlice, X: ToSlice> Bitwise<M, X> {
    /// Returns a new `Bitwise` instance that first masks the value it's applied to with `mask`
    /// and then performs xor with the value in `xor`.
    pub fn new(mask: M, xor: X) -> Self {
        Self { mask, xor }
    }
}

impl<M: ToSlice, X: ToSlice> Expression for Bitwise<M, X> {
    fn get_raw_name() -> *const c_char {
        b"bitwise\0" as *const _ as *const c_char
    }

    fn to_expr(&self, _rule: &Rule) -> *mut sys::nftnl_expr {
        unsafe {
            let expr = try_alloc!(sys::nftnl_expr_alloc(Self::get_raw_name()));

            let mask = self.mask.to_slice();
            let xor = self.xor.to_slice();
            assert!(mask.len() == xor.len());
            let len = mask.len() as u32;

            sys::nftnl_expr_set_u32(
                expr,
                sys::NFTNL_EXPR_BITWISE_SREG as u16,
                libc::NFT_REG_1 as u32,
            );
            sys::nftnl_expr_set_u32(
                expr,
                sys::NFTNL_EXPR_BITWISE_DREG as u16,
                libc::NFT_REG_1 as u32,
            );
            sys::nftnl_expr_set_u32(expr, sys::NFTNL_EXPR_BITWISE_LEN as u16, len);

            sys::nftnl_expr_set(
                expr,
                sys::NFTNL_EXPR_BITWISE_MASK as u16,
                mask.as_ref() as *const _ as *const c_void,
                len,
            );
            sys::nftnl_expr_set(
                expr,
                sys::NFTNL_EXPR_BITWISE_XOR as u16,
                xor.as_ref() as *const _ as *const c_void,
                len,
            );

            expr
        }
    }
}

#[macro_export]
macro_rules! nft_expr_bitwise {
    (mask $mask:expr,xor $xor:expr) => {
        $crate::expr::Bitwise::new($mask, $xor)
    };
}
