// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Relative Error Quantiles (REQ) sketch.
//!
//! A port of the C++ `req_sketch`, based on
//! [Relative Error Streaming Quantiles][paper] by Cormode, Karnin, Liberty, Thaler and Veselý.
//!
//! Unlike the other quantiles sketches in this library, which give a uniform rank error at every
//! rank, REQ gives an error proportional to how far the rank sits from a chosen end of the
//! distribution. Configured for high rank accuracy the error at rank `r` is proportional to
//! `1 - r`, which keeps p99.9 and p99.99 distinguishable.
//!
//! [paper]: https://arxiv.org/abs/2004.01668

mod compactor;

/// Minimum value of parameter K.
pub const MIN_K: u16 = 4;

/// Default value of parameter K, matching the Java implementation.
pub const DEFAULT_K: u16 = 12;

/// Initial number of sections each compactor is divided into.
pub(crate) const INIT_NUM_SECTIONS: u8 = 3;

/// Multiplier applied when deriving a compactor's nominal capacity.
pub(crate) const MULTIPLIER: u32 = 2;

/// Constant term of the rank error model, used where the relative term would understate error.
pub(crate) const FIXED_RSE_FACTOR: f64 = 0.084;

/// Returns the coefficient of the relative term of the rank error model.
pub(crate) fn relative_rse_factor() -> f64 {
    (0.0512 / INIT_NUM_SECTIONS as f64).sqrt()
}
