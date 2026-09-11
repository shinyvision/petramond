use super::*;
#[test]
fn stream_and_rejection_sampling_match_known_sequences() {
    let cases = [
        (
            0,
            [0.730967787376657, 0.24053641567148587, 0.6374174253501083],
            [140, 156, 1, 715581077, 0, 0, 64],
        ),
        (
            2345,
            [0.9336903394987087, 0.5615676452116335, 0.0543526215823642],
            [64, 174, 9, 438797488, 1, 0, 135],
        ),
        (
            u64::MAX,
            [
                0.26894263088050496,
                0.012269981921235296,
                0.6620844841121951,
            ],
            [106, 183, 16, 121412731, 1, 0, 393],
        ),
    ];
    for (seed, units, bounded) in cases {
        let mut rng = Lcg48::new(seed);
        for unit in units {
            assert_eq!(rng.unit(), unit);
        }
        for (limit, expected) in [256, 255, 17, 1073741825, 2, 1, 999]
            .into_iter()
            .zip(bounded)
        {
            assert_eq!(rng.bounded(limit), expected);
        }
    }
}
