use std::{convert::Infallible, path::PathBuf};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use cursive::CursiveRunnable;
use git_record::{
    testing::{CursiveTestingBackend, CursiveTestingEvent},
    FileContent, Hunk, HunkChangedLine, RecordState, Recorder,
};

fn bench_record(c: &mut Criterion) {
    c.bench_function("toogle line", |b| {
        let before_hunk = HunkChangedLine {
            is_selected: false,
            line: "foo\n".to_string(),
        };
        let after_hunk = HunkChangedLine {
            is_selected: false,
            line: "foo\n".to_string(),
        };
        let files = vec![(
            PathBuf::from("foo"),
            FileContent::Text {
                file_mode: (0o100644, 0o100644),
                hunks: vec![Hunk::Changed {
                    before: vec![before_hunk; 1000],
                    after: vec![after_hunk; 1000],
                }],
            },
        )];
        let record_state = RecordState { files };
        b.iter_batched(
            || {
                let siv = CursiveRunnable::new::<Infallible, _>(move || {
                    Ok(CursiveTestingBackend::init(vec![
                        CursiveTestingEvent::Event(' '.into()),
                        CursiveTestingEvent::Event(' '.into()),
                        CursiveTestingEvent::Event('q'.into()),
                    ]))
                });
                let siv = siv.into_runner();
                let recorder = Recorder::new(record_state.clone());
                (recorder, siv)
            },
            |(recorder, siv)| recorder.run(siv),
            BatchSize::PerIteration,
        )
    });
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_record,
);
criterion_main!(benches);
