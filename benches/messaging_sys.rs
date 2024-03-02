use std::alloc::System;

mod messaging;

#[global_allocator]
static ALLOCATOR: System = System;

criterion::criterion_main!(messaging::cases);
