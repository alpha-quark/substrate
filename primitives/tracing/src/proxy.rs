// Copyright 2020 Parity Technologies (UK) Ltd.
// This file is part of Substrate.

// Substrate is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

// Substrate is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.

// You should have received a copy of the GNU General Public License
// along with Substrate.  If not, see <http://www.gnu.org/licenses/>.

use std::cell::RefCell;
use rental;
use tracing::info_span;

/// Used to identify a proxied WASM trace
pub const WASM_TRACE_IDENTIFIER: &'static str = "WASM_TRACE";
/// Used to extract the real `target` from the associated values of the span
pub const WASM_TARGET_KEY: &'static str = "proxied_wasm_target";
/// Used to extract the real `name` from the associated values of the span
pub const WASM_NAME_KEY: &'static str = "proxied_wasm_name";

const MAX_SPANS_LEN: usize = 1000;

thread_local! {
	static PROXY: RefCell<TracingProxy> = RefCell::new(TracingProxy::new());
}

/// Create and enter a `tracing` Span, returning the span id,
/// which should be passed to `exit_span(id)` to signal that the span should exit.
pub fn create_registered_span(target: &str, name: &str) -> u64 {
	PROXY.with(|proxy| proxy.borrow_mut().create_span(target, name))
}

/// Exit a span by dropping it along with it's associated guard.
pub fn exit_span(id: u64) {
	PROXY.with(|proxy| proxy.borrow_mut().exit_span(id));
}

rental! {
	pub mod rent_span {
		#[rental]
		pub struct SpanAndGuard {
			span: Box<tracing::Span>,
			guard: tracing::span::Entered<'span>,
		}
	}
}

/// Requires a tracing::Subscriber to process span traces,
/// this is available when running with client (and relevant cli params).
pub struct TracingProxy {
	next_id: u64,
	spans: Vec<(u64, rent_span::SpanAndGuard)>,
}

impl Drop for TracingProxy {
	fn drop(&mut self) {
		while let Some((_, mut sg)) = self.spans.pop() {
			sg.rent_all_mut(|s| { s.span.record("is_valid_trace", &false); });
		}
	}
}

impl TracingProxy {
	pub fn new() -> TracingProxy {
		let spans: Vec<(u64, rent_span::SpanAndGuard)> = Vec::new();
		TracingProxy {
			next_id: 0,
			spans,
		}
	}
}

/// For spans to be recorded they must be registered in `span_dispatch`.
impl TracingProxy {
	// The identifiers `wasm_target` and `wasm_name` must match their associated const,
	// WASM_TARGET_KEY and WASM_NAME_KEY.
	fn create_span(&mut self, proxied_wasm_target: &str, proxied_wasm_name: &str) -> u64 {
		let span = info_span!(WASM_TRACE_IDENTIFIER, is_valid_trace = true, proxied_wasm_target, proxied_wasm_name);
		self.next_id += 1;
		let sg = rent_span::SpanAndGuard::new(
			Box::new(span),
			|span| span.enter(),
		);
		self.spans.push((self.next_id, sg));
		let spans_len = self.spans.len();
		if spans_len > MAX_SPANS_LEN {
			// This is to prevent unbounded growth of Vec and could mean one of the following:
			// 1. Too many nested spans, or MAX_SPANS_LEN is too low.
			// 2. Not correctly exiting spans due to drop impl not running (panic in runtime)
			// 3. Not correctly exiting spans due to misconfiguration / misuse
			log::warn!("MAX_SPANS_LEN exceeded, removing oldest span, recording `is_valid_trace = false`");
			let mut sg = self.spans.remove(0).1;
			sg.rent_all_mut(|s| { s.span.record("is_valid_trace", &false); });
		}
		self.next_id
	}

	fn exit_span(&mut self, id: u64) {
		match self.spans.pop() {
			Some(v) => {
				let mut last_span_id = v.0;
				while id < last_span_id {
					log::warn!("Span ids not equal! id parameter given: {}, last span: {}", id, last_span_id);
					if let Some(mut s) = self.spans.pop() {
						last_span_id = s.0;
						if id != last_span_id {
							s.1.rent_all_mut(|s| { s.span.record("is_valid_trace", &false); });
						}
					} else {
						log::warn!("Span id not found {}", id);
						return;
					}
				}
			}
			None => {
				log::warn!("Span id: {} not found", id);
				return;
			}
		}
	}
}
