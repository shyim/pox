use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pox_semver::constraint::php_version_compare;
use pox_semver::{Semver, VersionParser};

fn bench_php_version_compare(c: &mut Criterion) {
    let cases = [
        ("1.2.3", "1.2.4", "<"),
        ("2.4.0-alpha", "2.4.0", "<"),
        ("2.1.0.0-dev", "2.1.0.0", "<"),
        ("1.2.3+build.1", "1.2.3+build.2", "=="),
        ("1.0.0", "1", ">="),
        ("dev-master", "dev-feature", "!="),
        ("1.2.3-rc1", "1.2.3", "<"),
        ("1.2.3-pl1", "1.2.3", ">"),
    ];

    c.bench_function("php_version_compare", |b| {
        b.iter(|| {
            for (a, bver, op) in cases {
                black_box(php_version_compare(
                    black_box(a),
                    black_box(bver),
                    black_box(op),
                ));
            }
        })
    });
}

fn bench_normalize(c: &mut Criterion) {
    let parser = VersionParser::new();
    let versions = [
        "v1.2.3",
        "1.2.3-beta.1",
        "2.4.0+build.5",
        "1.2.x-dev",
        "dev-master",
        "2020.04.20",
        "1.2.3-rc1",
        "1.2.3-pl1",
        "1.2.3-alpha2",
    ];

    c.bench_function("normalize_versions", |b| {
        b.iter(|| {
            for version in versions {
                black_box(parser.normalize(black_box(version)).ok());
            }
        })
    });
}

fn bench_parse_constraints(c: &mut Criterion) {
    let parser = VersionParser::new();
    let constraints = [
        ">=1.2.3 <2.0.0",
        "^1.2.3 || ~2.4",
        "1.2.* || 2.*",
        "1.2.3 - 2.0.0",
        "~1.2.1 >=1.2.3",
        "!=1.5.0, !=1.5.1",
        ">1.0 <3.0 || >=4.0",
        "dev-master || 1.2.x-dev",
    ];

    c.bench_function("parse_constraints", |b| {
        b.iter(|| {
            for constraint in constraints {
                black_box(parser.parse_constraints(black_box(constraint)).ok());
            }
        })
    });
}

fn bench_satisfies(c: &mut Criterion) {
    let cases = [
        ("1.2.3", "^1.2.0"),
        ("1.2.3-beta", "^1.2.3"),
        ("2.4.5", "~2.4"),
        ("1.2.3", ">=1.2.3 <2.0.0"),
        ("1.9999.9999", "<2.0.0"),
        ("dev-master", "dev-master"),
        ("2.1.0.0-dev", "<2.1.0.0"),
        ("1.2.3", "1.2.* || 2.*"),
    ];

    c.bench_function("semver_satisfies", |b| {
        b.iter(|| {
            for (version, constraint) in cases {
                black_box(Semver::satisfies(
                    black_box(version),
                    black_box(constraint),
                ));
            }
        })
    });
}

fn bench_satisfies_parsed(c: &mut Criterion) {
    let cases = [
        "1.2.3",
        "1.2.3-beta",
        "2.4.5",
        "1.9999.9999",
        "dev-master",
        "2.1.0.0-dev",
        "1.9.0",
        "2.0.0",
    ];

    let parsed = Semver::parse_constraints("^1.2").expect("parse constraints");

    c.bench_function("semver_satisfies_parsed", |b| {
        b.iter(|| {
            for version in cases {
                black_box(Semver::satisfies_parsed(black_box(version), black_box(&parsed)));
            }
        })
    });
}

fn bench_sort(c: &mut Criterion) {
    let versions = vec![
        "1.0",
        "0.1",
        "0.1.1",
        "3.2.1",
        "2.4.0-alpha",
        "2.4.0",
        "dev-foo",
        "dev-master",
        "50.2",
        "1.2.3",
        "2.4.5",
        "2.4.5-rc1",
    ];

    c.bench_function("semver_sort", |b| {
        b.iter(|| {
            black_box(Semver::sort(black_box(&versions)));
        })
    });
}

criterion_group!(
    benches,
    bench_php_version_compare,
    bench_normalize,
    bench_parse_constraints,
    bench_satisfies,
    bench_satisfies_parsed,
    bench_sort
);
criterion_main!(benches);
