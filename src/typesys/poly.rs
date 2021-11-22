use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fmt::Debug,
    ops::{Add, Mul},
    sync::Arc,
};

use ethnum::U256;
use tap::Tap;

use crate::containers::List;

use super::{ConstExpr, Variable};

#[derive(Default, Clone, PartialEq, Eq)]
pub struct Polynomial<CVar: Variable> {
    terms: BTreeMap<BTreeMap<CVar, usize>, U256>,
}

impl<CVar: Variable> Polynomial<CVar> {
    /// Creates the zero polynomial.
    pub fn zero() -> Self {
        Self {
            terms: BTreeMap::new(),
        }
    }

    /// Finds all integers that when plugged into the polynomial, produce rhs.
    pub fn solve(&self, rhs: U256) -> List<U256> {
        if rhs > U256::from(u16::MAX) {
            log::warn!("cannot solve polynomials at huge values yet");
            return List::new();
        }
        log::trace!("solving {:?} = {}", self, rhs);
        if self.terms.keys().any(|b| b.len() > 1) || !self.terms.keys().any(|b| b.len() == 1) {
            log::trace!("cannot solve multivariate polynomial");
            List::new()
        } else {
            let constant_term = self
                .terms
                .get(&BTreeMap::new())
                .copied()
                .unwrap_or_default();
            let leading_term = self.terms.values().last().copied().unwrap_or_default();
            let fixed_constant_term = constant_term
                .checked_sub(rhs)
                .unwrap_or_else(|| rhs - constant_term);
            log::trace!(
                "constant_term = {}, leading_term = {}",
                fixed_constant_term,
                leading_term
            );
            // Apply the rational root theorem
            let pp = factors(fixed_constant_term);
            let qq = factors(leading_term);
            log::trace!("pp = {:?}, qq = {:?}", pp, qq);
            pp.iter()
                .copied()
                .map(|p| qq.iter().copied().map(move |q| (p, q)))
                .flatten()
                .chain(std::iter::once((0u8.into(), 1u8.into())))
                .filter_map(|(p, q)| {
                    let r = p / q;
                    if p % q != 0 {
                        None
                    } else if self.evaluate(|_| r) == rhs {
                        Some(r)
                    } else {
                        None
                    }
                })
                .collect()
        }
    }

    /// Evaluate at a given point.
    pub fn evaluate(&self, f: impl Fn(CVar) -> U256) -> U256 {
        self.terms
            .iter()
            .map(|(k, v)| {
                k.iter()
                    .map(|(k, v)| f(k.clone()).pow(*v as u32))
                    .product::<U256>()
                    * v
            })
            .sum()
    }

    /// Checked subtraction.
    pub fn checked_sub(mut self, rhs: Self) -> Option<Self> {
        for (k, v) in rhs.terms {
            let w = self.terms.entry(k).or_default();
            *w = w.checked_sub(v)?;
        }
        Some(self)
    }
}

/// factorize a number. currently just trial divisions.
fn factors(i: U256) -> List<U256> {
    let mut toret = List::new();
    let mut d = U256::from(1u8);
    loop {
        if d > i {
            return toret;
        }
        if i % d == 0 {
            toret.push_back(d);
        }
        d += 1;
    }
}

impl<CVar: Variable> From<&ConstExpr<CVar>> for Polynomial<CVar> {
    fn from(cexpr: &ConstExpr<CVar>) -> Self {
        match cexpr {
            ConstExpr::Literal(v) => Self {
                terms: maplit::btreemap! {
                    maplit::btreemap!{
                    } => *v,
                },
            },
            ConstExpr::Var(cv) => Self {
                terms: maplit::btreemap! {
                    maplit::btreemap!{
                        cv.clone() => 1
                    } => 1u8.into(),
                },
            },
            ConstExpr::Plus(a, b) => Self::from(a.as_ref()) + Self::from(b.as_ref()),
            ConstExpr::Mult(a, b) => Self::from(a.as_ref()) * Self::from(b.as_ref()),
        }
        .tap_mut(|s| s.terms.retain(|_, v| v > &mut U256::from(0u8)))
    }
}

impl<CVar: Variable> Into<ConstExpr<CVar>> for Polynomial<CVar> {
    fn into(self) -> ConstExpr<CVar> {
        self.terms
            .into_iter()
            .fold(ConstExpr::from(0), |a, (b, coeff)| {
                ConstExpr::Plus(
                    a.into(),
                    ConstExpr::Mult(
                        Arc::new(coeff.into()),
                        b.into_iter()
                            .fold(ConstExpr::from(1), |a, b| {
                                ConstExpr::Mult(
                                    a.into(),
                                    std::iter::repeat(b.0)
                                        .take(b.1)
                                        .fold(ConstExpr::from(1), |a, b| {
                                            ConstExpr::Mult(a.into(), ConstExpr::Var(b).into())
                                        })
                                        .into(),
                                )
                            })
                            .into(),
                    )
                    .into(),
                )
            })
    }
}

impl<CVar: Variable> PartialOrd<Self> for Polynomial<CVar> {
    /// TODO: cases like x < x^2 are not handled yet.
    /// Generally, that will be handled by attempting to find a coefficient with an equal or higher degree and comparing with that
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        if self <= other && other <= self {
            Some(Ordering::Equal)
        } else if self <= other {
            Some(Ordering::Less)
        } else if other <= self {
            Some(Ordering::Greater)
        } else {
            None
        }
    }

    fn le(&self, other: &Self) -> bool {
        log::trace!("comparing {:?} <=? {:?}", self, other);
        self.terms.iter().all(|(k, v)| {
            log::trace!("lt {:?}", k);
            let ov = other.terms.get(k).copied().unwrap_or_default();
            v <= &ov
        })
    }
}

impl<CVar: Variable> Add<Self> for Polynomial<CVar> {
    type Output = Self;
    fn add(mut self, rhs: Self) -> Self::Output {
        for (k, v) in rhs.terms {
            *self.terms.entry(k).or_default() += v;
        }
        self
    }
}

impl<CVar: Variable> Mul<Self> for Polynomial<CVar> {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        // cartesian-product the shit out of this
        let mut res_terms = BTreeMap::new();
        for (my_var, my_coeff) in self.terms.iter() {
            for (their_var, their_coeff) in rhs.terms.iter() {
                let res_var = var_multiply(my_var.clone(), their_var);
                *res_terms.entry(res_var).or_default() += *my_coeff * *their_coeff;
            }
        }
        res_terms.retain(|_, &mut v| v > 0);
        Self { terms: res_terms }
    }
}

fn var_multiply<CVar: Variable>(
    mut a: BTreeMap<CVar, usize>,
    b: &BTreeMap<CVar, usize>,
) -> BTreeMap<CVar, usize> {
    for (k, v) in b.iter() {
        *a.entry(k.clone()).or_default() += v;
    }
    a
}

impl<CVar: Variable> Debug for Polynomial<CVar> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for (k, v) in self.terms.iter() {
            if !std::mem::replace(&mut first, false) {
                std::fmt::Display::fmt(" + ", f)?;
            }
            v.fmt(f)?;
            for (k, v) in k.iter() {
                k.fmt(f)?;
                std::fmt::Display::fmt(&num_to_superscript(*v), f)?;
            }
        }
        Ok(())
    }
}

fn num_to_superscript(num: usize) -> String {
    num.to_string()
        .chars()
        .map(|c| match c {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            _ => unreachable!(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::containers::Symbol;

    use super::Polynomial;

    #[test]
    fn simple_poly() {
        let mut poly: Polynomial<Symbol> = Polynomial::zero();
        poly.terms = maplit::btreemap! {
            maplit::btreemap!{
                Symbol::from("x") => 2,
            } => 1u8.into(),
        };
        dbg!(poly.solve(4u8.into()));
    }
}