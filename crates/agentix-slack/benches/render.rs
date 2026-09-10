use agentix_domain::OutboundView;
use agentix_slack::render_view;
use std::{hint::black_box, time::Instant};

fn main() {
    for size in [4096, 1_048_576, 8_388_608] {
        let view = OutboundView::text("Benchmark", "a<&>中文😀".repeat(size / 14));
        for _ in 0..5 {
            black_box(render_view(black_box(&view)).unwrap());
        }
        let start = Instant::now();
        for _ in 0..100 {
            black_box(render_view(black_box(&view)).unwrap());
        }
        println!(
            "{{\"input_bytes\":{},\"iterations\":100,\"ns_per_render\":{}}}",
            view.body.len(),
            start.elapsed().as_nanos() / 100
        );
    }
}
