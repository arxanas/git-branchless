use std::{convert::Infallible, path::PathBuf};

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use cursive::CursiveRunnable;
use git_record::{
    testing::{CursiveTestingBackend, CursiveTestingEvent},
    FileState, RecordState, Recorder, Section, SectionChangedLine,
};

fn bench_record(c: &mut Criterion) {
    c.bench_function("toggle line", |b| {
        let before_line = SectionChangedLine {
            is_selected: false,
            line: "foo\n".to_string(),
        };
        let after_line = SectionChangedLine {
            is_selected: false,
            line: "foo\n".to_string(),
        };
        let file_states = vec![(
            PathBuf::from("foo"),
            FileState::Text {
                file_mode: (0o100644, 0o100644),
                sections: vec![Section::Changed {
                    before: vec![before_line; 1000],
                    after: vec![after_line; 1000],
                }],
            },
        )];
        let record_state = RecordState { file_states };
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
