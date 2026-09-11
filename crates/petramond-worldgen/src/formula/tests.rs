use super::*;

fn expression(text: &str) -> Expression {
    serde_json::from_str(text).unwrap()
}

#[test]
fn vertical_scratch_matches_independent_samples_in_any_order() {
    let bindings = vec![
        ("plan".into(), expression(r#"["noise2","x","z",30]"#)),
        (
            "height_signal".into(),
            expression(r#"["noise3","x","y","z",17]"#),
        ),
    ];
    let formula = Formula::compile(
        &bindings,
        &[
            expression(r#"["add","plan",["mul","height_signal","y"]]"#),
            expression(r#"["lt","y","center_y"]"#),
        ],
    )
    .unwrap();
    let mut inputs = Inputs([-83.0, 0.0, 19.0, 0.0, -4.0, 0.0, 12.0, 30.0, 70.0, 63.0]);
    let mut column = formula.column(inputs);
    for y in [16.0, -32.0, -4.0, -3.0, 0.0, 17.0, -32.0] {
        inputs.0[1] = y;
        assert_eq!(column.at::<2>(y), formula.sample::<2>(inputs));
    }
}

#[test]
fn references_and_operator_arity_are_checked_before_evaluation() {
    for text in [r#""absent""#, r#"["add",2]"#, r#"["unknown",1,2]"#, "[]"] {
        assert!(Formula::compile(&[], &[expression(text)]).is_err());
    }
    assert!(Formula::compile(&[("x".into(), Expression::Number(3.0))], &[]).is_err());
}

#[test]
fn compilation_shares_expressions_and_prunes_unused_work() {
    let bindings = vec![
        ("unused".into(), expression(r#"["noise3","x","y","z",17]"#)),
        ("sum".into(), expression(r#"["add","x",["mul",2,3]]"#)),
    ];
    let formula = Formula::compile(
        &bindings,
        &[
            expression(r#"["add","sum",["ceil","y"]]"#),
            expression(r#"["add",["add","x",["mul",2,3]],["ceil","y"]]"#),
        ],
    )
    .unwrap();
    assert_eq!(formula.outputs[0], formula.outputs[1]);
    assert!(formula
        .column_ops
        .iter()
        .chain(formula.vertical_ops.iter())
        .all(|&i| !matches!(formula.nodes[i].op, Op::Noise3 | Op::Mul)));
    let mut column = formula.column(Inputs([5.0; 10]));
    assert_eq!(column.at::<2>(-2.8), [9.0, 9.0]);
    assert_eq!(column.at::<2>(2.1), [14.0, 14.0]);
}

#[test]
fn parameter_batches_match_independent_columns_in_any_height_order() {
    let formula = Formula::compile(
        &[
            ("shared".into(), expression(r#"["noise3","x","y","z",29]"#)),
            (
                "width".into(),
                expression(r#"["add","radius",["noise2","x","z",11]]"#),
            ),
        ],
        &[
            expression(r#"["mul","shared",["sub","y","center_y"]]"#),
            expression(r#"["div",["sub","x","center_x"],"width"]"#),
        ],
    )
    .unwrap();
    let shared = Inputs([-33.0, 0.0, 49.0, 0.0, 0.0, 0.0, 1.0, 32.0, 70.0, 63.0]);
    let members: Vec<_> = (0..7)
        .map(|i| {
            let mut p = shared;
            p.0[3] = f64::from(i * 13);
            p.0[4] = f64::from(i * 5 - 19);
            p.0[6] = f64::from(i * 7 + 1);
            p
        })
        .collect();
    let batch = formula.batch(&[3, 4, 6]);
    let mut column = batch.column(shared, &members);
    for y in [-30.0, 0.0, 16.0, -1.0, -30.0] {
        column.at::<2>(y, |i, values| {
            let mut point = members[i];
            point.0[1] = y;
            assert_eq!(values, formula.sample::<2>(point));
        });
        let mut visits = 0;
        let found = column.find_map::<2, _>(y, |i, values| {
            visits += 1;
            (i == 3).then_some(values)
        });
        let mut point = members[3];
        point.0[1] = y;
        assert_eq!(found, Some(formula.sample::<2>(point)));
        assert_eq!(visits, 4, "accepted members stop evaluation");
    }
}

#[test]
fn seeded_samples_keep_their_seed_through_folding_and_batching() {
    let formula = Formula::compile(&[], &[
        expression(r#"["random",17,-19,3,42]"#),
        expression(r#"["random",17,"x","y","z"]"#),
        expression(r#"{"perlin":{"salt":[12,34],"first_octave":-3,"amplitudes":[1,0.5]},"at":["x","y","z"]}"#),
    ]).unwrap();
    let point = Inputs([-19.0, 3.0, 42.0, 0.0, 0.0, 0.0, 1.0, 1.0, 70.0, 64.0]);
    let first = formula.column_seeded(123, point).at::<3>(3.0);
    let second = formula.column_seeded(456, point).at::<3>(3.0);
    assert_eq!(first[0], first[1]);
    assert_eq!(second[0], second[1]);
    assert_ne!(first[0], second[0]);
    assert_ne!(first[2], second[2]);
    assert_eq!(first, formula.column_seeded(123, point).at::<3>(3.0));
    formula
        .batch(&[])
        .column_seeded(123, point, &[point])
        .at::<3>(3.0, |_, value| assert_eq!(value, first));
}

/// A column scan is the point evaluation laid out per height: one operator
/// dispatch per node instead of per cell, and a sampling operator answered
/// once per run of equal operands. Every lane must still be the point's value.
#[test]
fn column_scans_match_point_samples_lane_for_lane() {
    let formula = Formula::compile(
        &[
            ("wall".into(), expression(r#"["noise3","x",["trunc",["mul","y",0.1]],"z",40]"#)),
            ("band".into(), expression(r#"{"perlin":{"salt":[5,6],"first_octave":-4,"amplitudes":[1,1]},"at":["x",0,"z"]}"#)),
            ("dice".into(), expression(r#"["random",99,"x","y","z"]"#)),
        ],
        &[
            expression(r#"["lt",["mul","band","dice"],["mul","wall",["sub","y","center_y"]]]"#),
            expression(r#"["select",["gt","wall",0],"radius",["add","dice","height"]]"#),
        ],
    )
    .unwrap();
    let inputs = Inputs([13.0, 0.0, -7.0, 20.0, -4.0, 1.0, 12.0, 30.0, 70.0, 63.0]);
    let ys: Vec<f64> = (-44..=-19).map(f64::from).collect();
    let mut scan = formula.scan(77);
    let mut out = Vec::new();
    scan.run(inputs, &ys, &mut out);
    assert_eq!(out.len(), ys.len());
    for (lane, &y) in ys.iter().enumerate() {
        assert_eq!(out[lane], formula.column_seeded(77, inputs).at::<2>(y));
    }
    // A reused scan carries nothing over between columns or seeds.
    let other = Inputs([-2.0, 0.0, 9.0, 20.0, -4.0, 1.0, 12.0, 30.0, 70.0, 63.0]);
    scan.run(other, &ys[3..9], &mut out);
    for (lane, &y) in ys[3..9].iter().enumerate() {
        assert_eq!(out[lane], formula.column_seeded(77, other).at::<2>(y));
    }
}

/// Heights a box's bounds prove no member can cut are dead; anything the
/// bounds leave open stays alive, including a squared unknown offset,
/// which is never negative but otherwise unbounded.
#[test]
fn dead_heights_follow_from_the_box_bounds_alone() {
    let formula = Formula::compile(
        &[
            ("dx".into(), expression(r#"["sub","x","center_x"]"#)),
            (
                "reach".into(),
                expression(r#"["sub",3,["abs",["sub","y","center_y"]]]"#),
            ),
        ],
        &[expression(
            r#"["lt",["mul","dx","dx"],["mul","reach","radius"]]"#,
        )],
    )
    .unwrap();
    let batch = formula.batch_predicate(&[3]);
    let scan = batch.scan(1);
    let mut bounds: [Option<[f64; 2]>; 10] = [None; 10];
    bounds[0] = Some([0.0, 15.0]);
    bounds[4] = Some([10.0, 10.0]);
    bounds[6] = Some([5.0, 5.0]);
    let ys: Vec<f64> = (0..=20).map(f64::from).collect();
    let mut dead = Vec::new();
    scan.dead_lanes(&bounds, &ys, &mut dead);
    // Reach is positive only within three of the centre height.
    let expected: Vec<bool> = (0..=20).map(|y| !(8..=12).contains(&y)).collect();
    assert_eq!(dead, expected);
    // An unbounded radius could flip the comparison's sign: nothing is dead.
    bounds[6] = None;
    scan.dead_lanes(&bounds, &ys, &mut dead);
    assert!(dead.iter().all(|&d| !d));
}

/// A batch scan agrees with the per-member column, whole-column and lane
/// by lane, and a predicate batch prunes a column only when its first
/// output is exactly zero for every member at every height.
#[test]
fn batch_scans_match_members_and_prune_only_dead_predicates() {
    let formula = Formula::compile(
        &[("depth".into(), expression(r#"["sub","surface","y"]"#))],
        &[
            expression(r#"["and",["and",["le",0,"depth"],["lt","surface","sea"]],["lt",["sub","x","center_x"],["mul","radius","depth"]]]"#),
            expression(r#"["add","center_x","radius"]"#),
        ],
    )
    .unwrap();
    let shared = Inputs([5.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 1.0, 40.0, 63.0]);
    let members: Vec<_> = (0..5)
        .map(|i| {
            let mut p = shared;
            p.0[3] = f64::from(i * 3);
            p.0[6] = f64::from(i + 1);
            p
        })
        .collect();
    let ys: Vec<f64> = (30..=45).map(f64::from).collect();
    let batch = formula.batch_predicate(&[3, 6]);
    let mut scan = batch.scan(9);
    scan.run(shared, &members, &ys);
    for (m, member) in members.iter().enumerate() {
        for (lane, &y) in ys.iter().enumerate() {
            let mut point = *member;
            point.0[1] = y;
            assert_eq!(scan.output::<2>(m, lane), formula.sample::<2>(point));
        }
    }
    scan.run_shared(shared, &ys);
    for (m, member) in members.iter().enumerate() {
        for (lane, &y) in ys.iter().enumerate() {
            let mut point = *member;
            point.0[1] = y;
            assert_eq!(
                scan.member_lane::<2>(m, *member, lane),
                formula.sample::<2>(point)
            );
        }
    }
    // Land columns (surface above sea) make a shared conjunct false at every
    // height: every member reads zero without being evaluated.
    let mut land = shared;
    land.0[8] = 70.0;
    let land_members: Vec<_> = members
        .iter()
        .map(|m| {
            let mut p = *m;
            p.0[8] = 70.0;
            p
        })
        .collect();
    scan.run(land, &land_members, &ys);
    for m in 0..members.len() {
        assert_eq!(scan.output::<2>(m, 0), [0.0, 0.0]);
    }
    // The plain batch never prunes, so its second output stays meaningful.
    let plain = formula.batch(&[3, 6]);
    let mut plain = plain.scan(9);
    plain.run(land, &land_members, &ys);
    assert_eq!(plain.output::<2>(2, 0), [0.0, 6.0 + 3.0]);
}
