use super::*;
use crate::host::fake::rows::{AIR, BEDROCK, DIRT};
use crate::worker::{act, hands, pocket};

#[test]
fn air_above_water_is_not_a_working_stance_below_a_completed_floor() {
    let target = [6, 0, 6];
    let mut cells = vec![(target, "petramond:stone")];
    for x in 1..=11 {
        for z in 1..=11 {
            cells.push(([x, 2, z], "petramond:stone"));
        }
    }
    let (mut session, id) = Session::site("Work beneath a floor", [13, 4, 13], &cells);
    let water = resolve_block("petramond:water").unwrap();
    session.world.fill([1, -1, 1], [11, -1, 11], water);
    session.world.fill([1, 2, 1], [11, 2, 11], STONE);
    session.world.set([0, 0, 0], STONE);
    session.world.set([0, 1, 1], STONE);
    session.world.set(target, DIRT);
    session.job(id);
    session.golem(id, HOME, HOME);
    at_work(&mut session, id, |_, _, _| {});
    at_work(&mut session, id, |_, _, job| {
        assert_eq!(job.access_plan.score(), Some(1));
        assert!(!job.access_plan.candidates().is_empty());
    });
    for x in 5..=7 {
        for z in 5..=7 {
            session.world.set([x, 2, z], AIR);
        }
    }
    at_work(&mut session, id, |_, _, job| {
        assert_eq!(job.access_plan.score(), Some(0));
    });
    for x in 5..=7 {
        for z in 5..=7 {
            if [x, 2, z] != [7, 2, 6] {
                session.world.set([x, 2, z], STONE);
            }
        }
    }
    at_work(&mut session, id, |ctx, projects, job| {
        let project = projects.get(id).unwrap().clone();
        if let Some(survey) = &mut job.survey {
            let units: Vec<_> = (0..job.design.units.len()).collect();
            survey.measure(&job.design, &units);
        }
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(job.access_plan.score(), Some(0));
        let roof = job.design.unit_at([7, 2, 6]).unwrap();
        assert!(
            pocket::seals(ctx, job, roof).unwrap().is_some(),
            "keep the last view from actual ground until the work below is finished"
        );
    });
}

#[test]
fn unavailable_native_ground_never_becomes_a_zero_sealed_score() {
    let (mut session, id, _) = working(AT);
    session.world.route_budget(Some(0));
    at_work(&mut session, id, |ctx, projects, job| {
        assert_eq!(job.access_plan.score(), None);
        assert!(!pocket::refresh(ctx, job, projects.get(id).unwrap()));
        assert!(!pocket::placement(
            ctx,
            job,
            [0, 0, 0],
            &CellRecord::of("petramond:stone")
        ));
    });
}

#[test]
fn all_regions_are_unsealed_before_building_and_shared_access_stays_open() {
    let gate_a = [3, 0, 4];
    let dirt = [3, 0, 6];
    let behind = [3, 0, 7];
    let gate_b = [7, 0, 4];
    let head_a = [3, 1, 4];
    let head_b = [7, 1, 4];
    let water = [7, 0, 5];
    let cells =
        [gate_a, head_a, dirt, behind, gate_b, head_b, water].map(|c| (c, "petramond:stone"));
    let (mut session, id) = Session::site("Sealed rooms", [9, 2, 9], &cells);
    session.world.fill([2, -1, 4], [4, 2, 8], BEDROCK);
    session.world.fill([6, -1, 4], [8, 2, 6], BEDROCK);
    for c in [gate_a, head_a, gate_b, head_b] {
        session.world.set(c, STONE);
    }
    for c in [[3, 0, 5], behind] {
        session.world.set(c, AIR);
    }
    for c in [[3, 1, 5], [3, 1, 6], [3, 1, 7], [7, 1, 5]] {
        session.world.set(c, AIR);
    }
    session.world.set(dirt, DIRT);
    session
        .world
        .set(water, resolve_block("petramond:water").unwrap());
    session.job(id);
    let golem = session.golem(id, HOME, [3, 0, 2]);
    session
        .world
        .give(ContainerAddress::Entity(golem), "petramond:stone", 8);
    let body = session.body(golem);
    let a = unit(&session, id, gate_a);
    let a_head = unit(&session, id, head_a);
    let b = unit(&session, id, gate_b);
    let b_head = unit(&session, id, head_b);
    let obstruction = unit(&session, id, dirt);
    let far = unit(&session, id, behind);

    at_work(&mut session, id, |ctx, projects, job| {
        assert_eq!(job.access_plan.score(), Some(3));
        assert_eq!(
            job.access_plan.candidates().first().map(|c| c.0),
            Some(Task::Reopen(a))
        );
        assert!(
            job.access_plan.opens(Task::Reopen(b)),
            "the other region is already in the plan"
        );
        let step = plan(ctx, projects, job, &body);
        assert!(
            matches!(step, Step::Centre { task: Task::Reopen(o), .. } if o == a),
            "{step:?}"
        );
    });

    session.world.set(gate_a, AIR);
    at_work(&mut session, id, |ctx, projects, job| {
        act::settle(ctx, projects, job, Task::Reopen(a));
        let project = projects.get(id).unwrap().clone();
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(
            job.access_plan.score(),
            Some(3),
            "a one-block opening does not give the worker room to enter"
        );
        assert!(job.access_plan.opens(Task::Reopen(a_head)));
    });
    session.world.set(head_a, AIR);
    at_work(&mut session, id, |ctx, projects, job| {
        act::settle(ctx, projects, job, Task::Reopen(a_head));
        let project = projects.get(id).unwrap().clone();
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(job.access_plan.score(), Some(2));
        assert!(
            job.access_plan.opens(Task::Unit(obstruction)),
            "clearing a shared route is recovery work"
        );
        assert!(job.access_plan.opens(Task::Reopen(b)));
        job.crew.access.way_in = Some((vec![behind, water], ctx.now));
        assert!(!pocket::placement(
            ctx,
            job,
            gate_a,
            &CellRecord::of("petramond:stone")
        ));
    });

    session.world.set(dirt, AIR);
    at_work(&mut session, id, |ctx, projects, job| {
        act::settle(ctx, projects, job, Task::Unit(obstruction));
        let project = projects.get(id).unwrap().clone();
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(job.access_plan.score(), Some(1));
        let candidates = job.access_plan.candidates();
        assert!(candidates.iter().any(|c| c.0 == Task::Reopen(b)));
        assert!(candidates.iter().any(|c| c.0 == Task::Reopen(b_head)));
        assert!(candidates
            .iter()
            .all(|c| matches!(c.0, Task::Reopen(o) if o == b || o == b_head)));
        assert!(
            viability(ctx, job, &body, Task::Unit(far), body.pos) == Viable::Waits(SEALED_WAIT),
            "accessible work waits until all regions are open"
        );
    });

    for c in [gate_b, head_b] {
        session.world.set(c, AIR);
    }
    at_work(&mut session, id, |ctx, projects, job| {
        act::settle(ctx, projects, job, Task::Reopen(b));
        act::settle(ctx, projects, job, Task::Reopen(b_head));
        let project = projects.get(id).unwrap().clone();
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(job.access_plan.score(), Some(0));
        assert!(
            job.crew.access.way_in.is_none(),
            "opened regions need no further approach digs"
        );
        assert!(
            pocket::seals(ctx, job, a_head).unwrap().is_some(),
            "an opening serves the whole region until its remaining work is finished"
        );
        assert_eq!(
            hands::lay(
                ctx,
                job,
                &body,
                head_a,
                CellRecord::of("petramond:stone"),
                true,
                None
            ),
            hands::Lay::Refused(ActionRefusal::Changed),
            "the execution seam rejects resealing too"
        );
    });

    for cell in [behind, dirt] {
        session.world.set(cell, STONE);
    }
    at_work(&mut session, id, |ctx, projects, job| {
        act::settle(ctx, projects, job, Task::Unit(far));
        act::settle(ctx, projects, job, Task::Unit(obstruction));
        let project = projects.get(id).unwrap().clone();
        assert!(pocket::refresh(ctx, job, &project));
        assert_eq!(
            pocket::seals(ctx, job, a_head),
            Some(None),
            "finished regions can be closed"
        );
        assert!(
            pocket::seals(ctx, job, b).unwrap().is_some(),
            "the other region still contains work"
        );
    });
}

#[test]
fn deferred_openings_do_not_starve_later_regions_out_of_the_search_window() {
    let rooms = crate::worker::tuning::window::STANCE_SEARCHES + 1;
    let mut cells = Vec::new();
    for n in 0..rooms {
        let x = 3 * n as i32 + 1;
        for cell in [[x, 0, 1], [x, 1, 1], [x, 0, 2]] {
            cells.push((cell, "petramond:stone"));
        }
    }
    let (mut session, id) = Session::site("Many closed regions", [rooms as i32 * 3, 2, 4], &cells);
    session
        .world
        .fill([-5, -1, -5], [rooms as i32 * 3 + 5, -1, 8], STONE);
    for n in 0..rooms {
        let x = 3 * n as i32 + 1;
        session.world.fill([x - 1, -1, 1], [x + 1, 2, 3], BEDROCK);
        for c in [[x, 0, 1], [x, 1, 1]] {
            session.world.set(c, STONE);
        }
        session
            .world
            .set([x, 0, 2], resolve_block("petramond:water").unwrap());
        session.world.set([x, 1, 2], AIR);
    }
    session.job(id);
    let golem = session.golem(id, HOME, HOME);
    let body = session.body(golem);
    at_work(&mut session, id, |_, _, _| {});
    at_work(&mut session, id, |ctx, projects, job| {
        assert_eq!(job.access_plan.score(), Some(rooms));
        let candidates = job.access_plan.candidates();
        assert!(candidates.len() > crate::worker::tuning::window::STANCE_SEARCHES);
        let last = *candidates.last().unwrap();
        for (task, _) in &candidates[..candidates.len() - 1] {
            job.crew.deferrals.defer(*task, ctx.now + 1000);
        }
        let mut round = Round {
            project: projects.get(id).unwrap().clone(),
            mode: Mode::Building,
            candidates: Vec::new(),
            wants_items: false,
            waiting: false,
            walks: Vec::new(),
            wants_perch: Vec::new(),
            spots: Vec::new(),
            future: Vec::new(),
            supplies: super::super::supply::Requests::default(),
        };
        access::recover(ctx, projects, job, &body, &mut round);
        assert_eq!(round.candidates, vec![last]);
    });
}

#[test]
fn unread_access_never_reports_a_zero_score_or_allows_placement() {
    let (mut session, id, golem) = working(AT);
    let body = session.body(golem);
    session.world.unload([1, 0, 1], [1, 0, 1]);
    at_work(&mut session, id, |ctx, projects, job| {
        job.access_plan.changed(&[[1, 0, 1]], false);
        let project = projects.get(id).unwrap().clone();
        assert!(!pocket::refresh(ctx, job, &project));
        assert_eq!(job.access_plan.score(), None);
        assert_eq!(
            hands::lay(
                ctx,
                job,
                &body,
                [0, 0, 0],
                CellRecord::of("petramond:stone"),
                true,
                None
            ),
            hands::Lay::Refused(ActionRefusal::Changed)
        );
    });
}
