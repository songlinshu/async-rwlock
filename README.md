# async-rwlock

[![Build](https://github.com/stjepang/async-rwlock/workflows/Build%20and%20test/badge.svg)](
https://github.com/stjepang/async-rwlock/actions)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](
https://github.com/stjepang/async-rwlock)
[![Cargo](https://img.shields.io/crates/v/async-rwlock.svg)](
https://crates.io/crates/async-rwlock)
[![Documentation](https://docs.rs/async-rwlock/badge.svg)](
https://docs.rs/async-rwlock)

An async reader-writer lock.

The locking stragegy is fair: neither readers nor writers will be starved, assuming the task
executor is also fair.

## Examples

```rust
use async_rwlock::RwLock;

let lock = RwLock::new(5);

// Multiple read locks can be held at a time.
let r1 = lock.read().await;
let r2 = lock.read().await;
assert_eq!(*r1, 5);
assert_eq!(*r2, 5);
drop((r1, r2));

// Only one write lock can be held at a time.
let mut w = lock.write().await;
*w += 1;
assert_eq!(*w, 6);
```

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

#### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.
