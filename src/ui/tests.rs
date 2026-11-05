use super::*;
#[test]
fn failed_preferences_writes_are_throttled_and_recover() {
    let files = preferences::tests::Fixture::new();
    std::fs::write(files.path(), b"blocking parent").unwrap();
    let destination = files.path().join("settings.json");
    let mut app = ExplorerApp::empty();
    app.preferences_path = Some(destination.clone());
    app.appearance = Appearance::Dark;
    app.persist_preferences(false);
    assert!(app.preferences_write_failed);
    let failed_at = app.last_preferences_write;
    for _ in 0..5 {
        app.persist_preferences(false);
    }
    assert_eq!(app.last_preferences_write, failed_at);
    let context = egui::Context::default();
    let output = frame(&mut app, &context, vec![]);
    assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()==i18n::text(app.language,"preferences_save_failed"))));
    std::fs::remove_file(files.path()).unwrap();
    app.last_preferences_write = Instant::now() - Duration::from_secs(2);
    app.persist_preferences(false);
    assert!(!app.preferences_write_failed);
    assert_eq!(Preferences::load(&destination).appearance, Appearance::Dark);
}
#[test]
fn unknown_sizes_never_look_like_verified_empty_objects_in_any_language() {
    for language in 0..i18n::TAGS.len() {
        assert_eq!(
            measure(language, scan::Measure::default()),
            i18n::bytes(language, 0)
        );
        let unknown = measure(
            language,
            scan::Measure {
                unknown: 1,
                ..Default::default()
            },
        );
        assert!(unknown.starts_with(i18n::text(language, "unknown")));
        assert!(!unknown.contains(&i18n::bytes(language, 0)));
        assert_eq!(
            measure(
                language,
                scan::Measure {
                    known: 125739,
                    ..Default::default()
                }
            ),
            i18n::bytes(language, 125739)
        );
        assert!(
            measure(
                language,
                scan::Measure {
                    known: 10,
                    unknown: 1,
                    ..Default::default()
                }
            )
            .starts_with(&i18n::bytes(language, 10))
        );
    }
}
#[test]
fn incomplete_directory_explains_the_limit_and_can_be_scanned_separately() {
    let files = preferences::tests::Fixture::new();
    let directory = files.path().with_extension("directory");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("payload"), vec![1; 125739]).unwrap();
    let mut app = fixture();
    let result = Arc::make_mut(app.result.as_mut().unwrap());
    result.nodes.truncate(2);
    result.limited = true;
    result.limit_reason = Some(scan::LimitReason::MemoryLimit);
    for node in &mut result.nodes {
        node.complete = false;
        node.logical = scan::Measure {
            unknown: 1,
            ..Default::default()
        };
    }
    result.nodes[1].path = Some(directory.clone());
    app.navigation.directory = 1;
    app.navigation.selected = Some(1);
    let context = egui::Context::default();
    context.set_fonts(crate::fonts::definitions());
    theme::install(&context);
    for _ in 0..3 {
        frame(&mut app, &context, vec![]);
    }
    let output = context.run_ui(egui::RawInput::default(), |ui| app.details(ui));
    let mut output = output;
    output.textures_delta.clear();
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()==i18n::text(0,"scan_memory_limit"))));
    let pos = text_position(&output, i18n::text(0, "scan_folder_separately"));
    for pressed in [true, false] {
        let mut output = context.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                ..Default::default()
            },
            |ui| app.details(ui),
        );
        output.textures_delta.clear();
    }
    assert_eq!(app.scope.as_ref(), Some(&directory));
    let deadline = Instant::now() + Duration::from_secs(2);
    let result = loop {
        if let Some(result) = app.job.as_ref().and_then(scan::Job::take) {
            break result;
        }
        assert!(Instant::now() < deadline, "separate scan did not finish");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(result.nodes[0].logical.known, 125739);
    assert!(result.nodes[0].complete);
}
#[test]
fn settings_save_immediately_and_shutdown_flushes_pending_window_changes() {
    let files = preferences::tests::Fixture::new();
    let mut app = ExplorerApp::empty();
    app.preferences_path = Some(files.path());
    app.appearance = Appearance::Light;
    app.preferences.language = Some("fr".into());
    app.persist_preferences(false);
    let saved = Preferences::load(&files.path());
    assert_eq!(saved.language.as_deref(), Some("fr"));
    assert_eq!(saved.appearance, Appearance::Light);
    app.preferences.window = Some(crate::preferences::WindowState {
        position: Some([240, 160]),
        size: [900.0, 640.0],
        maximized: false,
    });
    app.last_preferences_write = Instant::now();
    app.persist_preferences(false);
    assert_eq!(Preferences::load(&files.path()).window, None);
    let expected = app.preferences.clone();
    drop(app);
    assert_eq!(Preferences::load(&files.path()), expected);
}
fn fixture() -> ExplorerApp {
    let mut app = ExplorerApp::empty();
    let nodes = [
        (None, "Scope", Kind::Directory, 120),
        (Some(0), "Photos", Kind::Directory, 100),
        (Some(1), "Trips", Kind::Directory, 100),
        (Some(2), "image.jpg", Kind::File, 100),
        (Some(0), "notes.txt", Kind::File, 20),
    ]
    .into_iter()
    .map(|(parent, name, kind, known)| scan::Node {
        parent,
        name: name.into(),
        locator: name.into(),
        native_locator: name.into(),
        kind,
        logical: scan::Measure {
            known,
            ..Default::default()
        },
        allocated: scan::Measure::default(),
        complete: true,
        hard_link: false,
        error: None,
        listing_complete: None,
        path: None,
    })
    .collect();
    app.result = Some(Arc::new(ScanResult {
        schema_version: 1,
        started_unix_seconds: 0,
        elapsed_millis: 1,
        scope: "Scope".into(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        memory: None,
        cross_filesystems: false,
        errors: 0,
        scope_identity: None,
        app_version: None,
        nodes,
    }));
    app.language = 0;
    app.phone_home = false;
    app
}
fn frame(
    app: &mut ExplorerApp,
    context: &egui::Context,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    frame_at(app, context, events, egui::vec2(1180.0, 900.0))
}
fn frame_at(
    app: &mut ExplorerApp,
    context: &egui::Context,
    events: Vec<egui::Event>,
    size: egui::Vec2,
) -> egui::FullOutput {
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ui| app.draw(ui),
    );
    output.textures_delta.clear();
    output
}
fn text_position(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing rendered text: {label}"))
}
fn click(app: &mut ExplorerApp, context: &egui::Context, pos: egui::Pos2) -> egui::FullOutput {
    for pressed in [true, false] {
        frame(
            app,
            context,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
        );
    }
    frame(app, context, vec![])
}
#[test]
fn phone_actions_live_in_more_and_export_formats_remain_available() {
    for language in 0..i18n::TAGS.len() {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.language = language;
        let size = egui::vec2(360.0, 800.0);
        for _ in 0..4 {
            frame_at(&mut app, &context, vec![], size);
        }
        let output = frame_at(&mut app, &context, vec![], size);
        let pos = text_position(&output, i18n::text(language, "phone_more"));
        for pressed in [true, false] {
            frame_at(
                &mut app,
                &context,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
                size,
            );
        }
        assert_eq!(app.phone_overlay, Some(PhoneOverlay::Actions));
        let output = frame_at(&mut app, &context, vec![], size);
        for key in ["rescan", "export_json", "export_csv"] {
            let pos = text_position(&output, i18n::text(language, key));
            assert!(pos.x >= 0.0 && pos.x < size.x && pos.y < size.y);
        }
    }
}
fn phone_click(app: &mut ExplorerApp, context: &egui::Context, label: &str) {
    let size = egui::vec2(360.0, 800.0);
    let output = frame_at(app, context, vec![], size);
    let pos = text_position(&output, label);
    for pressed in [true, false] {
        frame_at(
            app,
            context,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
            size,
        );
    }
}
#[test]
fn phone_folder_navigation_map_selection_and_back_preserve_the_result() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    let result = app.result.clone().unwrap();
    for _ in 0..4 {
        frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    }
    phone_click(&mut app, &context, "Photos");
    assert_eq!(
        app.navigation.directory, 1,
        "folders open with a single tap"
    );
    assert_eq!(app.navigation.selected, Some(1));
    phone_click(&mut app, &context, "Map");
    assert_eq!(app.phone_view, PhoneView::Map);
    let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    let color = tile_color(Some(&result.nodes[2]));
    let rect = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.fill == color => Some(r.rect),
            _ => None,
        })
        .expect("visible map tile");
    for pressed in [true, false] {
        frame_at(
            &mut app,
            &context,
            vec![
                egui::Event::PointerMoved(rect.center()),
                egui::Event::PointerButton {
                    pos: rect.center(),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                },
            ],
            egui::vec2(360.0, 800.0),
        );
    }
    assert_eq!(app.navigation.selected, Some(2));
    assert_eq!(app.phone_overlay, Some(PhoneOverlay::Details));
    app.phone_back();
    assert_eq!(app.phone_overlay, None);
    app.phone_back();
    assert_eq!(app.navigation.directory, 0);
    app.phone_back();
    assert!(app.phone_home);
    assert!(!app.phone_back(), "Back on Home is left to the platform");
    assert!(Arc::ptr_eq(&result, app.result.as_ref().unwrap()));
    phone_click(&mut app, &context, "Continue exploring");
    assert!(!app.phone_home);
    assert_eq!(app.phone_view, PhoneView::Map);
}
#[test]
fn phone_filter_opens_file_type_statistics_and_map_returns_to_folders() {
    for initial_view in [PhoneView::List, PhoneView::Map] {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let mut app = fixture();
        app.extensions = app.result.as_ref().unwrap().extensions();
        app.phone_view = initial_view;
        app.search.query = "old search".into();
        app.phone_overlay = Some(PhoneOverlay::Filter);
        for _ in 0..4 {
            frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
        }
        phone_click(&mut app, &context, "File types");
        assert!(app.navigation.page == Page::Extensions);
        assert_eq!(app.phone_view, PhoneView::List);
        assert!(app.search.query.is_empty());
        let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.text()=="jpg"))
        );
        assert!(
            !output.shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Text(t) if t.galley.text() == i18n::text(0, "search_hint"))),
            "File types must not expose an inactive search field"
        );
        phone_click(&mut app, &context, "Map");
        assert!(app.navigation.page == Page::Directory);
        assert_eq!(app.phone_view, PhoneView::Map);
    }
}
#[test]
fn phone_viewport_keeps_content_visible_in_all_languages_themes_and_large_text() {
    for language in 0..i18n::TAGS.len() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            context.set_theme(appearance.preference());
            let mut app = fixture();
            app.language = language;
            for (size, zoom) in [
                (egui::vec2(360.0, 800.0), 1.0),
                (egui::vec2(360.0, 800.0), 1.6),
                (egui::vec2(800.0, 400.0), 1.0),
            ] {
                context.set_zoom_factor(zoom);
                for view in [PhoneView::List, PhoneView::Map] {
                    app.phone_view = view;
                    for _ in 0..4 {
                        frame_at(&mut app, &context, vec![], size);
                    }
                    let output = frame_at(&mut app, &context, vec![], size);
                    let screen = context.content_rect();
                    for key in ["phone_list", "phone_map", "phone_more"] {
                        let pos = text_position(&output, i18n::text(language, key));
                        assert!(
                            screen.contains(pos),
                            "{key} offscreen: {language}, {size:?}, {zoom}"
                        );
                    }
                    if view == PhoneView::List {
                        let pos = text_position(&output, "Photos");
                        assert!(
                            pos.y < screen.height() * 0.65,
                            "content pushed below controls: {language}, {size:?}, zoom {zoom}, {pos:?}, {screen:?}"
                        );
                        assert!(screen.contains(pos));
                    } else {
                        let color = tile_color(Some(&app.result.as_ref().unwrap().nodes[1]));
                        let rect = output
                            .shapes
                            .iter()
                            .find_map(|s| match &s.shape {
                                egui::Shape::Rect(r) if r.fill == color => {
                                    Some(r.rect.intersect(s.clip_rect))
                                }
                                _ => None,
                            })
                            .expect("map visible without scrolling");
                        assert!(
                            rect.height() > screen.height() * 0.25,
                            "map squeezed: {rect:?}, {screen:?}"
                        );
                        assert!(
                            rect.bottom()
                                <= text_position(&output, i18n::text(language, "phone_map")).y
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn phone_rows_align_names_and_proportions_use_the_selected_size_measure() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    context.set_theme(Appearance::Light.preference());
    let mut app = fixture();
    let result = Arc::make_mut(app.result.as_mut().unwrap());
    for (index, known) in [(0, 1000), (1, 100), (4, 900)] {
        result.nodes[index].allocated = scan::Measure {
            known,
            ..Default::default()
        };
    }
    app.allocated = true;
    for _ in 0..4 {
        frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    }
    let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    let x = |name: &str| {
        output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == name => Some(t.pos.x),
                _ => None,
            })
            .unwrap()
    };
    assert!(
        (x("Photos") - x("notes.txt")).abs() < 1.0,
        "row names must share a left edge"
    );
    let fill = theme::accent(false).gamma_multiply(0.12);
    let widths: Vec<_> = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Rect(r) if r.fill == fill => Some(r.rect.width()),
            _ => None,
        })
        .collect();
    assert_eq!(widths.len(), 2);
    assert!(
        (widths[0] / widths[1] - 9.0).abs() < 0.01,
        "bars must use allocated bytes, not logical bytes"
    );
}
#[test]
fn phone_back_during_scan_cancels_without_entering_invisible_home() {
    let files = preferences::tests::Fixture::new();
    let mut app = ExplorerApp::empty();
    app.start(files.path().parent().unwrap().to_owned());
    assert!(!app.phone_home);
    assert!(app.phone_back());
    assert!(
        !app.phone_home,
        "progress remains visible until the partial result arrives"
    );
}
#[test]
fn settings_close_stays_inside_safe_area_after_rotation_and_large_text() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        context.set_theme(appearance.preference());
        let mut app = fixture();
        app.language = 1;
        app.settings_open = true;
        for (size, top, bottom, zoom) in [
            (egui::vec2(412.0, 915.0), 32.0, 88.0, 1.0),
            (egui::vec2(915.0, 412.0), 24.0, 74.0, 1.0),
            (egui::vec2(412.0, 915.0), 32.0, 88.0, 1.6),
        ] {
            context.set_zoom_factor(zoom);
            let size = size / zoom;
            let top = top / zoom;
            let bottom = bottom / zoom;
            for _ in 0..4 {
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        safe_area_insets: Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                            top,
                            bottom,
                            ..Default::default()
                        })),
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                output.textures_delta.clear();
            }
            let window =
                egui::containers::AreaState::load(&context, egui::Id::unique("settings-window"))
                    .unwrap();
            assert!(
                window.rect().top() >= top,
                "settings title is under the status bar"
            );
            assert!(
                window.rect().right() <= size.x,
                "settings close is off screen"
            );
        }
        // Feed Escape through a real frame rather than mutating widget state.
        frame_at(
            &mut app,
            &context,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            egui::vec2(412.0, 915.0),
        );
        assert!(!app.settings_open);
    }
}
#[test]
fn file_type_headers_sort_each_column_in_both_directions_without_rescanning() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    app.navigation.page = Page::Extensions;
    app.extensions = [("txt", 2, 20), ("jpg", 5, 10), ("", 1, 30), ("zip", 3, 20)]
        .into_iter()
        .map(|(name, count, known)| {
            (
                name.into(),
                count,
                scan::Measure {
                    known,
                    ..Default::default()
                },
            )
        })
        .collect();
    app.sort_extensions();
    let result = app.result.clone().unwrap();
    for (column, descending, expected) in [
        (ExtensionColumn::Name, false, ["", "jpg", "txt", "zip"]),
        (ExtensionColumn::Name, true, ["zip", "txt", "jpg", ""]),
        (ExtensionColumn::Size, true, ["", "txt", "zip", "jpg"]),
        (ExtensionColumn::Size, false, ["jpg", "txt", "zip", ""]),
        (ExtensionColumn::Files, true, ["jpg", "zip", "txt", ""]),
        (ExtensionColumn::Files, false, ["", "txt", "zip", "jpg"]),
    ] {
        for _ in 0..3 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        let heading = i18n::text(0, column.key());
        let label = if app.extension_sort.0 == column {
            format!("{heading} {}", if app.extension_sort.1 { "↓" } else { "↑" })
        } else {
            heading.to_owned()
        };
        let position = text_position(&output, &label);
        click(&mut app, &context, position);
        assert_eq!(app.extension_sort, (column, descending));
        assert_eq!(
            app.extensions
                .iter()
                .map(|row| row.0.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(Arc::ptr_eq(app.result.as_ref().unwrap(), &result));
        assert!(app.job.is_none());
    }
}
#[test]
fn file_type_sorting_preserves_unknown_sizes_and_orders_overflow_explicitly() {
    let mut app = fixture();
    let unknown = scan::Measure {
        unknown: 2,
        ..Default::default()
    };
    app.extensions = vec![
        ("unknown".into(), 2, unknown),
        ("empty".into(), 1, scan::Measure::default()),
        (
            "large".into(),
            3,
            scan::Measure {
                known: u64::MAX,
                ..Default::default()
            },
        ),
        (
            "overflow".into(),
            4,
            scan::Measure {
                known: u64::MAX,
                overflow: true,
                ..Default::default()
            },
        ),
    ];
    app.sort_extensions();
    assert_eq!(
        app.extensions
            .iter()
            .map(|row| row.0.as_str())
            .collect::<Vec<_>>(),
        ["overflow", "large", "empty", "unknown"]
    );
    app.extension_sort.1 = false;
    app.sort_extensions();
    assert_eq!(
        app.extensions
            .iter()
            .map(|row| row.0.as_str())
            .collect::<Vec<_>>(),
        ["empty", "unknown", "large", "overflow"]
    );
    assert_eq!(app.extensions[1].2, unknown);
}
#[test]
fn extension_columns_do_not_move_when_long_names_scroll_into_view() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    app.navigation.page = Page::Extensions;
    app.extensions = (0..200)
        .map(|i| {
            (
                if i < 12 {
                    format!("ext{i}")
                } else {
                    "long-extension-".repeat(40)
                },
                10,
                scan::Measure {
                    known: 4096,
                    ..Default::default()
                },
            )
        })
        .collect();
    for _ in 0..5 {
        frame(&mut app, &context, vec![]);
    }
    let first = frame(&mut app, &context, vec![]);
    let size_x = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == i18n::bytes(0, 4096) => {
                    Some(text.pos.x)
                }
                _ => None,
            })
            .expect("missing extension sizes")
    };
    let before = size_x(&first);
    frame(
        &mut app,
        &context,
        vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 450.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                source: egui::MouseWheelSource::Unknown,
                delta: egui::vec2(0.0, -900.0),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            },
        ],
    );
    for _ in 0..16 {
        frame(&mut app, &context, vec![]);
    }
    let after = frame(&mut app, &context, vec![]);
    assert!(after.shapes.iter().any(|shape| matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("long-extension-"))));
    assert!(
        (size_x(&after) - before).abs() < 1.0,
        "extension size column moved while scrolling"
    );
}
#[test]
fn map_divider_is_visible_and_draggable_in_both_themes() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        context.set_theme(appearance.preference());
        let mut app = fixture();
        for _ in 0..12 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        let grip = text_position(&output, i18n::text(0, "resize_map"));
        let panel_id = app.map_panel_id.unwrap();
        let state = || egui::containers::panel::PanelState::load(&context, panel_id).unwrap();
        let before = state().size().y;
        frame(
            &mut app,
            &context,
            vec![
                egui::Event::PointerMoved(grip),
                egui::Event::PointerButton {
                    pos: grip,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
        );
        frame(
            &mut app,
            &context,
            vec![egui::Event::PointerMoved(grip + egui::vec2(0.0, 40.0))],
        );
        frame(
            &mut app,
            &context,
            vec![egui::Event::PointerButton {
                pos: grip + egui::vec2(0.0, 40.0),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(
            (state().size().y - before).abs() > 20.0,
            "divider drag did not resize map"
        );
    }
}
#[test]
fn language_popup_stays_open_and_selects_a_language_in_settings() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let files = preferences::tests::Fixture::new();
    let mut app = ExplorerApp::empty();
    app.preferences_path = Some(files.path());
    app.language = 0;
    frame(&mut app, &context, vec![]);
    let output = frame(&mut app, &context, vec![]);
    let pos = text_position(&output, "Settings");
    click(&mut app, &context, pos);
    let output = frame(&mut app, &context, vec![]);
    assert!(app.settings_open);
    let pos = text_position(&output, "Dark");
    click(&mut app, &context, pos);
    let output = frame(&mut app, &context, vec![]);
    let pos = text_position(&output, "System (en)");
    click(&mut app, &context, pos);
    let output = frame(&mut app, &context, vec![]);
    let pos = text_position(&output, "ru");
    click(&mut app, &context, pos);
    assert_eq!(app.language, 1);
    assert!(app.settings_open);
    drop(app);
    let restored_context = egui::Context::default();
    let restored = ExplorerApp::new_with_preferences(
        &eframe::CreationContext::_new_kittest(restored_context.clone()),
        Some(files.path()),
    );
    assert_eq!(restored.language, 1);
    assert_eq!(restored.appearance, Appearance::Dark);
    assert_eq!(restored.preferences.language.as_deref(), Some("ru"));
    assert_eq!(restored_context.theme(), egui::Theme::Dark);
}
#[test]
fn tree_expansion_sorting_and_map_selection_preserve_hierarchy() {
    let mut app = fixture();
    app.refresh_rows();
    assert_eq!(app.tree.rows, vec![(0, 0), (1, 1), (4, 1)]);
    app.select(3);
    app.refresh_rows();
    assert_eq!(app.tree.rows, vec![(0, 0), (1, 1), (2, 2), (3, 3), (4, 1)]);
    app.tree.expanded.remove(&1);
    app.dirty = true;
    app.refresh_rows();
    assert_eq!(app.tree.rows.len(), 3);
    assert_eq!(app.navigation.selected, Some(3));
    app.sort = Sort::Name;
    app.dirty = true;
    app.refresh_rows();
    assert_eq!(app.tree.rows[1].0, 4);
    assert_eq!(app.map.objects.iter().flatten().count(), 2);
    app.navigate(1);
    app.refresh_rows();
    assert_eq!(app.tree.rows[0], (1, 0));
    app.back();
    assert_eq!(app.navigation.directory, 0);
}
#[test]
fn desktop_tree_stays_above_map_with_both_visible() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    frame(&mut app, &context, vec![]);
    for _ in 0..12 {
        frame(&mut app, &context, vec![]);
    }
    let output = frame(&mut app, &context, vec![]);
    let panel =
        egui::containers::panel::PanelState::load(&context, app.map_panel_id.unwrap()).unwrap();
    assert!(
        panel.size().y >= 200.0,
        "map panel shrank: {:?}",
        panel.size()
    );
    let tree = text_position(&output, "CONTENTS");
    let map = text_position(&output, "SPACE MAP");
    assert!(tree.y < map.y);
    assert!(map.y < 800.0);
}
#[test]
fn result_rows_do_not_overlap_map_after_resize_or_text_scaling() {
    for language in 0..i18n::TAGS.len() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            context.set_theme(appearance.preference());
            let mut app = fixture();
            app.language = language;
            app.select(3);
            for (size, zoom) in [
                (egui::vec2(1180.0, 900.0), 1.0),
                (egui::vec2(744.0, 684.0), 1.0),
                (egui::vec2(1180.0, 900.0), 1.6),
            ] {
                context.set_zoom_factor(zoom);
                for _ in 0..4 {
                    frame_at(&mut app, &context, vec![], size);
                }
                let mut output = frame_at(&mut app, &context, vec![], size);
                let folder_color = tile_color(Some(&app.result.as_ref().unwrap().nodes[1]));
                let map_position = |output: &egui::FullOutput| {
                    output.shapes.iter().find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.text() == i18n::text(language, "treemap") =>
                        {
                            Some(text.pos.y)
                        }
                        egui::Shape::Rect(rect) if rect.fill == folder_color => {
                            Some(rect.rect.top())
                        }
                        _ => None,
                    })
                };
                if map_position(&output).is_none() {
                    for _ in 0..8 {
                        output = frame_at(
                            &mut app,
                            &context,
                            vec![
                                egui::Event::PointerMoved(egui::pos2(24.0, size.y / zoom - 40.0)),
                                egui::Event::MouseWheel {
                                    unit: egui::MouseWheelUnit::Point,
                                    source: egui::MouseWheelSource::Unknown,
                                    phase: egui::TouchPhase::Move,
                                    delta: egui::vec2(0.0, -180.0),
                                    modifiers: Default::default(),
                                },
                            ],
                            size,
                        );
                        if map_position(&output).is_some() {
                            break;
                        }
                    }
                }
                let map_y = map_position(&output).unwrap_or_else(|| {
                    panic!("map is unreachable: {size:?}, zoom {zoom}, language {language}")
                });
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape
                        && ["Scope", "Photos", "Trips", "notes.txt"].contains(&text.galley.text())
                    {
                        let bounds = egui::Rect::from_min_size(text.pos, text.galley.size())
                            .intersect(shape.clip_rect);
                        if bounds.is_positive() {
                            assert!(
                                bounds.bottom() < map_y,
                                "tree overlaps map: {size:?}, zoom {zoom}, language {language}, {bounds:?} >= {map_y}"
                            );
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn selected_tree_row_has_a_background_without_hover() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    app.select(3);
    for _ in 0..3 {
        frame(&mut app, &context, vec![]);
    }
    let output = frame(
        &mut app,
        &context,
        vec![egui::Event::PointerMoved(egui::Pos2::ZERO)],
    );
    let text = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "image.jpg" => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            _ => None,
        })
        .next_back()
        .unwrap();
    let fill = context.style_of(context.theme()).visuals.selection.bg_fill;
    assert!(
        output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.fill == fill && rect.rect.contains(text))),
        "selected row has no persistent selection background"
    );
}
#[test]
fn nested_selection_marks_its_visible_containing_tile_and_path() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    app.select(3);
    for _ in 0..3 {
        frame(&mut app, &context, vec![]);
    }
    let output = frame(&mut app, &context, vec![]);
    assert_eq!(app.selected_tile(), Some(0));
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text().contains("Selected item: image.jpg"))));
    app.navigate(2);
    app.select(3);
    app.refresh_rows();
    assert_eq!(app.map.objects[app.selected_tile().unwrap()], Some(3));
    app.back();
    app.refresh_rows();
    assert_eq!(app.navigation.directory, 1);
}
#[test]
fn all_languages_render_at_phone_and_desktop_widths_in_each_theme() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    app.settings_open = true;
    for language in 0..i18n::TAGS.len() {
        app.language = language;
        for width in [320.0, 1180.0] {
            for appearance in [Appearance::Light, Appearance::Dark] {
                context.set_theme(appearance.preference());
                let mut output = context.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 820.0),
                        )),
                        ..Default::default()
                    },
                    |ui| app.draw(ui),
                );
                assert!(!output.shapes.is_empty());
                output.textures_delta.clear();
            }
        }
    }
}
#[test]
fn long_breadcrumbs_remain_one_path_row_in_all_languages_and_themes() {
    for language in 0..i18n::TAGS.len() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            for width in [320.0, 1180.0] {
                let context = egui::Context::default();
                context.set_fonts(crate::fonts::definitions());
                theme::install(&context);
                context.set_theme(appearance.preference());
                context.set_zoom_factor(1.6);
                let mut app = fixture();
                app.language = language;
                let result = Arc::make_mut(app.result.as_mut().unwrap());
                for index in 5..20 {
                    result.nodes.push(scan::Node {
                        parent: Some(if index == 5 { 2 } else { index - 1 }),
                        name: if index == 19 {
                            "current-folder".into()
                        } else {
                            format!("a-very-long-parent-directory-{index}")
                        },
                        locator: format!("/Scope/Photos/Trips/folder-{index}"),
                        native_locator: String::new(),
                        kind: Kind::Directory,
                        logical: scan::Measure::default(),
                        allocated: scan::Measure::default(),
                        complete: true,
                        hard_link: false,
                        error: None,
                        listing_complete: None,
                        path: None,
                    });
                }
                app.navigation.directory = 19;
                let mut path_rect = egui::Rect::NOTHING;
                let mut output = egui::FullOutput::default();
                for _ in 0..4 {
                    output = context.run_ui(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(width / 1.6, 500.0),
                            )),
                            ..Default::default()
                        },
                        |ui| {
                            path_rect = app.path_navigation(ui).rect;
                        },
                    );
                    output.textures_delta.clear();
                }
                assert!(
                    path_rect.right() <= width / 1.6 + 1.0,
                    "path exceeds viewport: {language} {width} {path_rect:?}"
                );
                assert!(
                    path_rect.height() < if width / 1.6 < 400.0 { 130.0 } else { 90.0 },
                    "path navigation grew beyond its compact layout: {path_rect:?}"
                );
                let mut centers = Vec::new();
                let mut current_visible = false;
                let mut path_label_visible = false;
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape {
                        let label = text.galley.text();
                        let center = text.pos + text.galley.size() * 0.5;
                        if label == "›"
                            || label == "current-folder"
                            || label.starts_with("a-very-long-parent-")
                        {
                            centers.push(center.y);
                        }
                        if label == "current-folder" {
                            current_visible = shape.clip_rect.contains(center);
                        }
                        if label == i18n::text(language, "path") {
                            path_label_visible = shape.clip_rect.contains(center);
                        }
                    }
                }
                assert!(
                    current_visible && path_label_visible,
                    "current folder or path label is hidden: {language} {width}"
                );
                assert!(
                    centers.iter().all(|y| (*y - centers[0]).abs() < 3.0),
                    "breadcrumb segments wrapped: {centers:?}"
                );
            }
        }
    }
}
#[test]
fn breadcrumb_links_navigate_to_ancestors_and_back_remains_separate() {
    let context = egui::Context::default();
    context.set_fonts(crate::fonts::definitions());
    theme::install(&context);
    let mut app = fixture();
    app.navigate(2);
    for _ in 0..4 {
        frame(&mut app, &context, vec![]);
    }
    let output = frame(&mut app, &context, vec![]);
    let photos = text_position(&output, "Photos");
    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text()=="Photos"
                && text.galley.job.sections.iter().any(|section| section.format.underline.width>0.0))));
    click(&mut app, &context, photos);
    assert_eq!(app.navigation.directory, 1);
    let output = frame(&mut app, &context, vec![]);
    let back = text_position(&output, "Back");
    click(&mut app, &context, back);
    assert_eq!(app.navigation.directory, 0);
}
#[test]
fn a_single_group_click_opens_all_25000_items_without_recursive_grouping() {
    let mut app = ExplorerApp::empty();
    app.language = 0;
    let nodes = (0..25001)
        .map(|i| scan::Node {
            parent: if i == 0 { None } else { Some(0) },
            name: format!("item-{i}"),
            locator: i.to_string(),
            native_locator: i.to_string(),
            kind: if i == 0 { Kind::Directory } else { Kind::File },
            logical: scan::Measure {
                known: if i == 0 {
                    25000 * 25001 / 2
                } else {
                    25001 - i as u64
                },
                ..Default::default()
            },
            allocated: scan::Measure::default(),
            complete: true,
            hard_link: false,
            error: None,
            listing_complete: None,
            path: None,
        })
        .collect();
    app.result = Some(Arc::new(ScanResult {
        schema_version: 1,
        started_unix_seconds: 0,
        elapsed_millis: 0,
        scope: String::new(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        memory: None,
        cross_filesystems: false,
        errors: 0,
        scope_identity: None,
        app_version: None,
        nodes,
    }));
    app.navigation.selected = Some(25000);
    app.refresh_rows();
    assert!(app.map.objects.len() <= 96);
    assert!(!app.map.grouped.is_empty());
    let grouped_count = app.map.grouped.len();
    assert!(
        app.map.objects[app.selected_tile().unwrap()].is_none(),
        "a selected small file must mark the grouped tile"
    );
    assert_eq!(grouped_count, 25000);
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    for label in [
        i18n::text(0, "grouped_items").to_owned(),
        format!(
            "{} ({})",
            i18n::text(0, "grouped_items"),
            i18n::integer(0, 25000)
        ),
    ] {
        for _ in 0..5 {
            frame(&mut app, &context, vec![]);
        }
        let output = frame(&mut app, &context, vec![]);
        let position = text_position(&output, &label);
        click(&mut app, &context, position);
        let output = frame(&mut app, &context, vec![]);
        assert_eq!(app.rows.len(), grouped_count);
        assert_eq!(app.map.group.as_ref().unwrap().len(), grouped_count);
        assert!(app.map.objects.is_empty());
        assert!(app.map.grouped.is_empty());
        assert_eq!(app.navigation.selected, Some(25000));
        let visible_names = output
            .shapes
            .iter()
            .filter(|shape| {
                matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text().starts_with("item-"))
            })
            .count();
        assert!(
            visible_names < 100,
            "group rows must remain virtualized: {visible_names}"
        );
        assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                egui::Shape::Text(text) if text.galley.text() == i18n::text(0,"treemap"))));
        app.sort = Sort::Name;
        app.dirty = true;
        app.refresh_rows();
        let mut expected = app.map.group.clone().unwrap();
        app.result.as_ref().unwrap().sort(&mut expected, Sort::Name);
        assert_eq!(app.rows, expected);
        app.search.query = "item-25000".into();
        app.dirty = true;
        app.refresh_rows();
        assert_eq!(app.rows, vec![25000]);
        assert_eq!(app.map.group.as_ref().unwrap().len(), grouped_count);
        let output = frame(&mut app, &context, vec![]);
        let back = text_position(&output, i18n::text(0, "back"));
        click(&mut app, &context, back);
        frame(&mut app, &context, vec![]);
        assert!(app.map.group.is_none());
        assert!(app.search.query.is_empty());
        assert_eq!(app.navigation.directory, 0);
        assert_eq!(app.navigation.selected, Some(25000));
        assert_eq!(app.map.grouped.len(), grouped_count);
        assert!(app.map.objects.len() <= 96);
    }
    let _ = context.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1180.0, 900.0),
            )),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
            ..Default::default()
        },
        |ui| {
            ui.memory_mut(|memory| memory.request_focus(ui.scope_id().with(("tile", 0usize))));
            app.map(ui, 260.0, false);
        },
    );
    app.refresh_rows();
    assert_eq!(app.map.group.as_ref().unwrap().len(), 25000);
    assert_eq!(app.rows.len(), 25000);
    assert!(app.map.grouped.is_empty());
    app.phone_home = false;
    for _ in 0..4 {
        frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    }
    let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
    let names = output
        .shapes
        .iter()
        .filter(
            |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.text().starts_with("item-")),
        )
        .count();
    assert!(
        names > 1 && names < 40,
        "the phone must virtualize the whole group, drew {names} names"
    );
    assert_eq!(app.rows.len(), 25000);
    assert_eq!(app.phone_view, PhoneView::List);
}
#[test]
fn phone_back_cancels_pending_selection_before_stale_navigation() {
    let _session = bridge::SESSION_TEST_LOCK.lock().unwrap();
    for grouped in [false, true] {
        let mut app = fixture();
        app.navigate(1);
        if grouped {
            app.map.group = Some(vec![2]);
        }
        // A second native picker clears the result before its metadata arrives.
        app.result = None;
        app.job = None;
        app.mobile_busy = true;
        app.phone_home = false;
        assert!(app.phone_back());
        assert!(!app.mobile_busy);
        assert!(app.phone_home);
        assert_eq!(app.notice, Some("cancelled"));
    }
}
#[test]
fn phone_global_results_use_root_title_size_and_proportions() {
    for search in [false, true] {
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        context.set_theme(Appearance::Light.preference());
        let mut app = fixture();
        let result = Arc::make_mut(app.result.as_mut().unwrap());
        result.nodes[0].logical.known = 1100;
        result.nodes[4].logical.known = 1000;
        app.navigate(1);
        if search {
            app.search.query = ".".into();
        } else {
            app.navigation.page = Page::Largest;
        }
        for _ in 0..4 {
            frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
        }
        let output = frame_at(&mut app, &context, vec![], egui::vec2(360.0, 800.0));
        assert!(
            app.rows.contains(&4),
            "global results include the sibling file"
        );
        assert_eq!(app.phone_scope(), 0);
        assert!(text_position(&output, "Scope").y < 80.0);
        let fill = theme::accent(false).gamma_multiply(0.12);
        let widths: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) if r.fill == fill => Some(r.rect.width()),
                _ => None,
            })
            .collect();
        assert_eq!(widths.len(), 2);
        assert!(
            (widths[0] / widths[1] - 10.0).abs() < 0.01,
            "global bars must use the root total instead of saturating at the folder total"
        );
        app.search.query.clear();
        app.navigation.page = Page::Directory;
        assert_eq!(app.phone_scope(), 1, "ordinary browsing retains its folder");
    }
}
#[test]
fn readable_uri_keeps_unicode_and_never_changes_machine_locator() {
    let uri = "content://provider/document/%D0%A0%D0%BE%D0%B1%D0%B8%D0%BD%2F%E6%97%A5%E6%9C%AC";
    assert_eq!(
        readable_locator(uri),
        "content://provider/document/Робин/日本"
    );
    assert!(uri.contains("%D0"));
    for value in ["%FF", "%x0", "%", "/literal%filename"] {
        assert_eq!(readable_locator(value), value);
    }
}
#[test]
fn unicode_queries_match_names_in_every_supported_language() {
    for (name, query) in [
        ("Робин Хобб", "РОБИН"),
        ("Straße", "straße"),
        ("日本語", "日本"),
        ("Français", "FRANÇAIS"),
        ("Español", "ESPAÑOL"),
        ("Português", "PORTUGUÊS"),
        ("English", "ENGLISH"),
    ] {
        let mut app = fixture();
        Arc::make_mut(app.result.as_mut().unwrap()).nodes[4].name = name.into();
        assert_eq!(
            app.result.as_ref().unwrap().search(query, true, Sort::Size),
            vec![4]
        );
    }
}
#[test]
fn phone_screens_respect_system_bars_and_explain_unavailable_allocation() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = fixture();
    Arc::make_mut(app.result.as_mut().unwrap()).nodes[0].allocated = scan::Measure {
        unknown: 5,
        ..Default::default()
    };
    app.language = 0;
    for (home, overlay) in [
        (true, None),
        (false, None),
        (false, Some(PhoneOverlay::Details)),
        (false, Some(PhoneOverlay::Actions)),
        (false, Some(PhoneOverlay::Filter)),
    ] {
        app.phone_home = home;
        app.phone_overlay = overlay;
        for _ in 0..4 {
            let mut output = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(412.0, 915.0),
                    )),
                    safe_area_insets: Some(egui::SafeAreaInsets(egui::epaint::MarginF32 {
                        top: 40.0,
                        bottom: 30.0,
                        ..Default::default()
                    })),
                    ..Default::default()
                },
                |ui| app.draw(ui),
            );
            output.textures_delta.clear();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape
                    && shape
                        .clip_rect
                        .intersects(egui::Rect::from_min_size(text.pos, text.galley.size()))
                {
                    assert!(
                        text.pos.y >= 40.0,
                        "text under system bar: {}",
                        text.galley.text()
                    );
                }
            }
            if overlay == Some(PhoneOverlay::Filter) {
                text_position(&output, i18n::text(0, "allocated_unavailable"));
                assert!(!app.allocated);
            }
        }
    }
}

// ----- Saved results and comparison -----

fn rendered_texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
            _ => None,
        })
        .collect()
}
/// A copy of a result after some objects changed, with aggregates recomputed.
fn variant(result: &ScanResult, edit: impl FnOnce(&mut Vec<scan::Node>)) -> ScanResult {
    let mut next = result.clone();
    for node in &mut next.nodes {
        if node.kind == Kind::Directory {
            node.logical = scan::Measure::default();
            node.allocated = scan::Measure::default();
        }
    }
    edit(&mut next.nodes);
    scan::aggregate(&mut next.nodes);
    next
}
fn app_with_saved_and_live() -> ExplorerApp {
    let mut app = fixture();
    let live = app.result.clone().unwrap();
    // Baseline: image.jpg was smaller, notes.txt did not exist, extra.bin was present.
    let saved = variant(&live, |nodes| {
        nodes[3].logical.known = 60;
        nodes[3].allocated.known = 60;
        nodes.truncate(5);
        nodes[4].logical.known = 20;
        let mut extra = nodes[4].clone();
        extra.name = "extra.bin".into();
        extra.locator = "extra.bin".into();
        extra.native_locator = "extra.bin".into();
        extra.logical.known = 400;
        extra.parent = Some(0);
        nodes.push(extra);
    });
    app.live = Some(live);
    app.saved = Some(SavedResult {
        result: Arc::new(saved),
        name: "photos-2026-09.json".into(),
    });
    app
}
fn settle(app: &mut ExplorerApp, context: &egui::Context, size: egui::Vec2) -> egui::FullOutput {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = frame_at(app, context, vec![], size);
        if app.changes.job.is_none() && app.open_job.is_none() {
            return frame_at(app, context, vec![], size).tap(output);
        }
        assert!(Instant::now() < deadline, "background job did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
trait Tap {
    fn tap(self, _: egui::FullOutput) -> Self;
}
impl Tap for egui::FullOutput {
    fn tap(self, _: egui::FullOutput) -> Self {
        self
    }
}
fn compare_in_app(app: &mut ExplorerApp, context: &egui::Context, size: egui::Vec2) {
    app.open_changes();
    app.changes.confirm_scope = true;
    app.start_compare();
    settle(app, context, size);
    assert!(
        app.changes.comparison.is_some(),
        "comparison did not finish"
    );
}

#[test]
fn saved_results_are_marked_distinctly_and_expose_no_actions_on_recorded_locations() {
    for language in 0..i18n::TAGS.len() {
        for (size, label) in [
            (egui::vec2(1180.0, 900.0), "desktop"),
            (egui::vec2(360.0, 800.0), "phone"),
        ] {
            let context = egui::Context::default();
            theme::install(&context);
            context.set_fonts(crate::fonts::definitions());
            let mut app = app_with_saved_and_live();
            app.language = language;
            app.show(View::Saved);
            for _ in 0..3 {
                frame_at(&mut app, &context, vec![], size);
            }
            let output = frame_at(&mut app, &context, vec![], size);
            let texts = rendered_texts(&output);
            let banner = i18n::text(language, "saved_banner");
            assert!(
                texts.iter().any(|t| t.starts_with(banner)),
                "banner missing: {language}, {label}"
            );
            assert!(
                app.scope.is_none() && app.provider_scope.is_none(),
                "nothing to rescan"
            );
            assert!(
                app.result
                    .as_ref()
                    .unwrap()
                    .nodes
                    .iter()
                    .all(|n| n.path.is_none())
            );
            assert!(app.job.is_none(), "viewing a saved result starts no scan");
            assert!(
                texts
                    .iter()
                    .all(|t| !t.starts_with("saved_") && !t.starts_with("cmp_"))
            );
        }
    }
}

#[test]
fn comparison_renders_labelled_statuses_in_all_languages_themes_sizes_and_text_scales() {
    for language in 0..i18n::TAGS.len() {
        for appearance in [Appearance::Light, Appearance::Dark] {
            for (size, zoom) in [
                (egui::vec2(1180.0, 900.0), 1.0),
                (egui::vec2(360.0, 800.0), 1.0),
                (egui::vec2(360.0, 800.0), 1.6),
            ] {
                let context = egui::Context::default();
                theme::install(&context);
                context.set_fonts(crate::fonts::definitions());
                context.set_theme(appearance.preference());
                context.set_zoom_factor(zoom);
                let mut app = app_with_saved_and_live();
                app.language = language;
                compare_in_app(&mut app, &context, size);
                let output = frame_at(&mut app, &context, vec![], size);
                let texts = rendered_texts(&output).join("\n");
                let t = |key| i18n::text(language, key);
                // The grew row and the missing row carry text labels, not only colors.
                assert!(texts.contains(t("cmp_grew")), "{language} {size:?} {zoom}");
                assert!(
                    texts.contains(t("cmp_removed")),
                    "{language} {size:?} {zoom}"
                );
                assert!(texts.contains("image.jpg") && texts.contains("extra.bin"));
                assert!(
                    texts.contains(&i18n::signed_bytes(language, 40))
                        || texts.contains(&i18n::signed_bytes(language, -400)),
                );
                let screen = context.content_rect();
                for shape in &output.shapes {
                    if let egui::Shape::Text(text) = &shape.shape
                        && shape
                            .clip_rect
                            .intersects(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    {
                        assert!(text.pos.x < screen.right() + 1.0, "text outside the screen");
                    }
                }
                for line in rendered_texts(&output) {
                    assert!(
                        !(line.starts_with("cmp_")
                            || line.starts_with("compare_")
                            || line.starts_with("saved_")),
                        "untranslated key shown: {line}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_comparison_screen_does_not_show_the_summary_of_a_previously_browsed_result() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(1180.0, 900.0);
    let mut app = app_with_saved_and_live();
    app.show(View::Saved);
    assert!(app.shows_saved_result());
    compare_in_app(&mut app, &context, size);
    let texts = rendered_texts(&frame_at(&mut app, &context, vec![], size));
    assert!(
        !texts
            .iter()
            .any(|t| t == i18n::text(0, "size_caveat") || t == i18n::text(0, "scope"))
    );
    // Native actions must still treat the browsed result as a file while Changes is on top.
    assert_eq!(app.view, View::Changes);
    assert!(app.shows_saved_result());
    app.show(View::Live);
    assert!(!app.shows_saved_result());
}

#[test]
fn comparison_summary_separates_confirmed_changes_from_the_unknown_part() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = app_with_saved_and_live();
    // A file whose size is unknown now: the total must not be presented as complete.
    Arc::make_mut(app.live.as_mut().unwrap()).nodes[4].logical = scan::Measure {
        unknown: 1,
        ..Default::default()
    };
    scan::aggregate(&mut Arc::make_mut(app.live.as_mut().unwrap()).nodes);
    let size = egui::vec2(1180.0, 900.0);
    compare_in_app(&mut app, &context, size);
    let comparison = app.changes.comparison.clone().unwrap();
    let totals = comparison.totals(Dimension::Logical);
    assert!(!totals.is_complete());
    let texts = rendered_texts(&frame_at(&mut app, &context, vec![], size)).join("\n");
    assert!(texts.contains(i18n::text(0, "cmp_tile_net_partial")));
    assert!(texts.contains(i18n::text(0, "cmp_net_partial")));
    assert!(!texts.contains(i18n::text(0, "cmp_net_complete")));
}

#[test]
fn insufficient_scope_requires_explicit_confirmation_and_swapping_reverses_the_direction() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(1180.0, 900.0);
    let mut app = app_with_saved_and_live();
    app.open_changes();
    let output = frame_at(&mut app, &context, vec![], size);
    let texts = rendered_texts(&output);
    assert!(
        texts
            .iter()
            .any(|t| t == i18n::text(0, "cmp_verdict_insufficient"))
    );
    assert!(texts.iter().any(|t| t == i18n::text(0, "compare_baseline")));
    // Without confirmation the job reports that confirmation is needed.
    app.start_compare();
    settle(&mut app, &context, size);
    assert!(app.changes.comparison.is_none());
    assert!(app.changes.failure.is_some());
    app.changes.confirm_scope = true;
    app.start_compare();
    settle(&mut app, &context, size);
    let forward = app
        .changes
        .comparison
        .clone()
        .unwrap()
        .totals(Dimension::Logical)
        .net();
    app.swap_direction();
    assert!(app.changes.comparison.is_none() && !app.changes.confirm_scope);
    app.changes.confirm_scope = true;
    app.start_compare();
    settle(&mut app, &context, size);
    assert_eq!(
        app.changes
            .comparison
            .clone()
            .unwrap()
            .totals(Dimension::Logical)
            .net(),
        -forward
    );
}

#[test]
fn opening_saved_files_in_sequence_keeps_only_the_latest_and_bad_files_do_not_replace_it() {
    let files = preferences::tests::Fixture::new();
    let base = files.path().with_extension("saved");
    std::fs::create_dir_all(&base).unwrap();
    let live = fixture().result.clone().unwrap();
    let write = |name: &str, bytes: u64| {
        let result = variant(&live, |nodes| nodes[3].logical.known = bytes);
        let path = base.join(name);
        std::fs::write(&path, serde_json::to_vec(&result).unwrap()).unwrap();
        path
    };
    let (first, second) = (write("first.json", 11), write("second.json", 22));
    let bad = base.join("bad.json");
    std::fs::write(&bad, b"{\"schema_version\":2,\"nodes\":[").unwrap();
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(1180.0, 900.0);
    let mut app = ExplorerApp::empty();
    app.language = 0;
    app.open_saved(first, false);
    app.open_saved(second, false);
    settle(&mut app, &context, size);
    assert_eq!(app.saved.as_ref().unwrap().name, "second.json");
    assert_eq!(
        app.saved.as_ref().unwrap().result.nodes[3].logical.known,
        22
    );
    assert_eq!(app.view, View::Saved);
    // A damaged file reports a clear error and leaves the open result untouched.
    app.open_saved(bad, false);
    settle(&mut app, &context, size);
    assert!(app.open_error.is_some());
    assert_eq!(app.saved.as_ref().unwrap().name, "second.json");
    let texts = rendered_texts(&frame_at(&mut app, &context, vec![], size));
    assert!(
        texts
            .iter()
            .any(|t| t == i18n::text(0, "import_error_malformed"))
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn phone_back_leaves_comparison_then_saved_result_without_losing_either() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(360.0, 800.0);
    let mut app = app_with_saved_and_live();
    app.show(View::Saved);
    compare_in_app(&mut app, &context, size);
    app.changes.selected = Some(1);
    app.phone_overlay = Some(PhoneOverlay::ChangeDetails);
    assert!(app.phone_back(), "details sheet closes first");
    assert_eq!(app.view, View::Changes);
    assert!(app.phone_back());
    assert_eq!(app.view, View::Live);
    assert!(app.saved.is_some() && app.live.is_some());
    assert!(
        app.changes.comparison.is_some(),
        "the comparison can be reopened"
    );
}

#[test]
fn showing_an_object_from_a_change_navigates_the_matching_result() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(1180.0, 900.0);
    let mut app = app_with_saved_and_live();
    compare_in_app(&mut app, &context, size);
    let comparison = app.changes.comparison.clone().unwrap();
    let removed = (0..comparison.nodes.len())
        .find(|&i| comparison.name(i) == "extra.bin")
        .unwrap();
    let old = comparison.nodes[removed].old.unwrap();
    app.goto(saved::Side::Baseline, old);
    assert_eq!(app.view, View::Saved);
    assert_eq!(app.navigation.selected, Some(old as usize));
    assert!(Arc::ptr_eq(
        app.result.as_ref().unwrap(),
        &comparison.baseline
    ));
    let grew = (0..comparison.nodes.len())
        .find(|&i| comparison.name(i) == "image.jpg")
        .unwrap();
    app.goto(saved::Side::Current, comparison.nodes[grew].new.unwrap());
    assert_eq!(app.view, View::Live);
    assert_eq!(
        app.navigation.directory, 2,
        "the containing folder is opened"
    );
    assert_eq!(app.navigation.selected, Some(3));
}

#[test]
fn a_new_scan_discards_the_comparison_but_keeps_the_saved_baseline() {
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let mut app = app_with_saved_and_live();
    compare_in_app(&mut app, &context, egui::vec2(1180.0, 900.0));
    let folder = preferences::tests::Fixture::new()
        .path()
        .with_extension("scan");
    std::fs::create_dir_all(&folder).unwrap();
    app.start(folder.clone());
    assert!(app.changes.comparison.is_none() && app.live.is_none());
    assert!(app.saved.is_some(), "the baseline survives a new scan");
    assert_eq!(app.view, View::Live);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_large_comparison_virtualizes_rows_and_stays_interactive() {
    // 120,000 objects per result; most are unchanged, about a sixth differ.
    let mut nodes = vec![scan::Node {
        parent: None,
        name: "data".into(),
        locator: "/data".into(),
        native_locator: "/data".into(),
        kind: Kind::Directory,
        logical: scan::Measure::default(),
        allocated: scan::Measure::default(),
        complete: true,
        hard_link: false,
        error: None,
        listing_complete: None,
        path: None,
    }];
    for i in 1..120_000_usize {
        let mut node = nodes[0].clone();
        node.parent = Some(if i % 50 == 0 { 0 } else { (i / 50) * 50 }.min(i - 1));
        node.kind = if i % 50 == 0 {
            Kind::Directory
        } else {
            Kind::File
        };
        node.name = format!("n{i}");
        node.locator = format!("/data/n{i}");
        node.native_locator = node.locator.clone();
        node.logical.known = (i as u64 * 7919) % 100_000;
        nodes.push(node);
    }
    scan::aggregate(&mut nodes);
    let old = ScanResult {
        schema_version: 1,
        started_unix_seconds: 1_790_000_000,
        elapsed_millis: 1,
        scope: "/data".into(),
        cancelled: false,
        limited: false,
        limit_reason: None,
        memory: None,
        cross_filesystems: false,
        errors: 0,
        scope_identity: None,
        app_version: None,
        nodes: nodes.clone(),
    };
    let new = variant(&old, |nodes| {
        for (i, node) in nodes.iter_mut().enumerate() {
            if node.kind == Kind::File && i % 6 == 0 {
                node.logical.known += 1 + (i as u64 % 977);
            }
        }
    });
    let mut app = ExplorerApp::empty();
    app.language = 0;
    app.phone_home = false;
    app.live = Some(Arc::new(new));
    app.saved = Some(SavedResult {
        result: Arc::new(old),
        name: "big.json".into(),
    });
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    let size = egui::vec2(1180.0, 900.0);
    let started = Instant::now();
    compare_in_app(&mut app, &context, size);
    let compare_time = started.elapsed();
    let comparison = app.changes.comparison.clone().unwrap();
    assert!(comparison.totals(Dimension::Logical).grew_count > 15_000);
    let started = Instant::now();
    let output = frame_at(&mut app, &context, vec![], size);
    let frame_time = started.elapsed();
    assert!(app.changes.rows.len() > 15_000);
    assert!(
        output.shapes.len() < 2_000,
        "only the visible rows are painted: {}",
        output.shapes.len()
    );
    // Switching to the folder tree and filtering also stay bounded.
    app.changes.mode = saved::ChangeMode::Tree;
    app.changes.query = "n1".into();
    app.changes.dirty = true;
    let started = Instant::now();
    frame_at(&mut app, &context, vec![], size);
    eprintln!(
        "large comparison fixture: {} objects per result, {} diff rows, {} events; compare {:?}, first frame {:?}, tree+filter frame {:?} (debug build)",
        comparison.baseline.nodes.len(),
        comparison.nodes.len(),
        app.changes.rows.len(),
        compare_time,
        frame_time,
        started.elapsed()
    );
}
fn search_fixture(count: usize) -> ExplorerApp {
    let mut app = fixture();
    let result = Arc::make_mut(app.result.as_mut().unwrap());
    let file = result.nodes[4].clone();
    for index in 0..count {
        let mut node = file.clone();
        node.name = format!("item-{index}.bin");
        node.locator = format!("/synthetic/{}", node.name);
        node.logical.known = (index % 37) as u64;
        node.allocated.known = (index % 19) as u64;
        result.nodes.push(node);
    }
    app
}

#[test]
fn pending_search_hides_stale_rows_instead_of_claiming_no_results() {
    for size in [egui::vec2(1180.0, 900.0), egui::vec2(430.0, 920.0)] {
        let mut app = fixture();
        app.refresh_rows();
        app.search.query = "missing".into();
        // Keep the debounce pending without depending on the test machine's speed.
        app.search.edited(Instant::now() + Duration::from_secs(60));
        app.dirty = true;
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        let output = frame_at(&mut app, &context, vec![], size);
        assert!(
            app.rows.is_empty(),
            "old directory rows must not be presented as matches"
        );
        assert!(
            !rendered_texts(&output)
                .iter()
                .any(|text| text == i18n::text(0, "no_results"))
        );
        app.search.edited(Instant::now() - search::DEBOUNCE);
        let output = frame_at(&mut app, &context, vec![], size);
        assert!(
            rendered_texts(&output)
                .iter()
                .any(|text| text == i18n::text(0, "no_results"))
        );
    }
}

#[test]
fn opening_a_background_group_does_not_expose_the_previous_directory() {
    let mut app = search_fixture(search::BACKGROUND_THRESHOLD);
    app.refresh_rows();
    assert!(!app.map.grouped.is_empty());
    app.open_group();
    app.refresh_rows();
    assert!(
        app.rows.is_empty(),
        "the previous directory is not the opened group"
    );
    assert!(app.map.objects.is_empty());
}

#[test]
fn selecting_a_background_match_updates_the_map_without_restarting_search() {
    let mut app = search_fixture(search::BACKGROUND_THRESHOLD);
    app.search.query = "notes".into();
    app.refresh_rows();
    let started = Instant::now();
    while app.rows != [4] {
        app.refresh_rows();
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::yield_now();
    }
    app.select(4);
    assert!(
        !app.dirty,
        "selection must not invalidate the completed search"
    );
    assert_eq!(app.map.selected, Some(0));
    app.refresh_rows();
    assert_eq!(app.rows, vec![4]);
    assert_eq!(app.map.selected, Some(0));
}

#[test]
fn hidden_phone_search_does_not_schedule_continuous_repaints() {
    for overlay in [None, Some(PhoneOverlay::Actions)] {
        let mut app = fixture();
        app.phone_home = overlay.is_none();
        app.phone_overlay = overlay;
        app.search.query = "notes".into();
        app.search.edited(Instant::now() - search::DEBOUNCE);
        let context = egui::Context::default();
        theme::install(&context);
        context.set_fonts(crate::fonts::definitions());
        for _ in 0..5 {
            frame_at(&mut app, &context, vec![], egui::vec2(430.0, 920.0));
        }
        let output = frame_at(&mut app, &context, vec![], egui::vec2(430.0, 920.0));
        assert!(output.viewport_output[&egui::ViewportId::ROOT].repaint_delay > Duration::ZERO);
        assert!(app.dirty);
    }
}

#[test]
fn background_filtering_preserves_unicode_scope_and_both_size_orders() {
    use std::sync::atomic::AtomicBool;
    let mut app = search_fixture(5000); // Exercise multiple merge runs.
    let result = Arc::make_mut(app.result.as_mut().unwrap());
    result.nodes[5].name = "ФОТО.JPG".into();
    for query in ["", "item", "фото", "SYNTHETIC", "missing"] {
        for files_only in [false, true] {
            for sort in [Sort::Name, Sort::Size] {
                for allocated in [false, true] {
                    let request = search::Request {
                        result: app.result.as_ref().unwrap().clone(),
                        query: query.into(),
                        group: None,
                        files_only,
                        sort,
                        allocated,
                    };
                    let mut expected = request.result.search(query, files_only, sort);
                    if allocated && sort == Sort::Size {
                        expected.sort_unstable_by(|a, b| {
                            request.result.nodes[*b]
                                .allocated
                                .known
                                .cmp(&request.result.nodes[*a].allocated.known)
                                .then(a.cmp(b))
                        });
                    }
                    assert_eq!(request.run(&AtomicBool::new(false)).unwrap(), expected);
                    assert!(request.run(&AtomicBool::new(true)).is_none());
                }
            }
        }
    }
    let request = search::Request {
        result: app.result.as_ref().unwrap().clone(),
        query: "item".into(),
        group: Some(vec![5, 6, 7]),
        files_only: false,
        sort: Sort::Name,
        allocated: false,
    };
    assert_eq!(request.run(&AtomicBool::new(false)).unwrap(), vec![6, 7, 5]);
}

#[test]
fn large_search_discards_superseded_and_queued_results_and_resets_on_navigation() {
    let mut app = search_fixture(search::BACKGROUND_THRESHOLD);
    app.refresh_rows();
    let result = app.result.as_ref().unwrap().clone();
    for query in ["item", "missing", "item-49999.bin"] {
        app.search.query = query.into();
        app.dirty = true;
        app.refresh_rows();
    }
    let started = Instant::now();
    while app.rows != [50004] {
        app.refresh_rows();
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::yield_now();
    }
    assert!(Arc::ptr_eq(app.result.as_ref().unwrap(), &result));
    app.search.query = "missing".into();
    app.dirty = true;
    app.refresh_rows();
    app.navigate(1);
    app.refresh_rows();
    assert_eq!(app.rows, vec![2]);
    for _ in 0..5 {
        app.refresh_rows();
    }
    assert_eq!(app.rows, vec![2]);
    // Replacing the displayed result also discards the pending search.
    app.search.start(search::Request {
        result: result.clone(),
        query: "missing".into(),
        group: None,
        files_only: false,
        sort: Sort::Name,
        allocated: false,
    });
    app.set_displayed(Some(result));
    app.refresh_rows();
    assert_eq!(app.navigation.directory, 0);
    assert_eq!(app.rows.len(), search::BACKGROUND_THRESHOLD + 2);
}

#[test]
#[ignore = "explicit performance measurement on a large synthetic tree"]
fn search_responsiveness_measurement() {
    let mut app = search_fixture(600_000);
    for query in ["item-", "item-599999", "missing"] {
        let started = Instant::now();
        let rows = app
            .result
            .as_ref()
            .unwrap()
            .search(query, false, Sort::Size);
        eprintln!(
            "search: objects={}, query={query}, matches={}, elapsed_us={}, release={}",
            app.result.as_ref().unwrap().nodes.len(),
            rows.len(),
            started.elapsed().as_micros(),
            !cfg!(debug_assertions)
        );
    }
    app.refresh_rows(); // Exclude the initial directory index and font atlas.
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    frame(&mut app, &context, vec![]);
    for (query, expected) in [("item-", 600_000), ("item-599999.bin", 1), ("missing", 0)] {
        app.search.query = query.into();
        app.search.edited(Instant::now() - search::DEBOUNCE);
        app.dirty = true;
        let started = Instant::now();
        let mut maximum = Duration::ZERO;
        loop {
            let tick = Instant::now();
            frame(&mut app, &context, vec![]);
            maximum = maximum.max(tick.elapsed());
            if !app.search.pending() && !app.dirty && app.rows.len() == expected {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(30));
            std::thread::sleep(Duration::from_millis(1));
        }
        eprintln!(
            "background search: query={query}, matches={expected}, total_us={}, max_ui_frame_us={}",
            started.elapsed().as_micros(),
            maximum.as_micros()
        );
    }
}
#[test]
#[ignore = "explicit performance measurement on a large synthetic tree"]
fn scan_memory_responsiveness_measurement() {
    use crate::scan::memory_tests::Generated;
    let mut app = ExplorerApp::empty();
    app.language = 0;
    app.phone_home = false;
    let context = egui::Context::default();
    theme::install(&context);
    context.set_fonts(crate::fonts::definitions());
    frame(&mut app, &context, vec![]); // Exclude initial font atlas work.
    app.job = Some(app.service.start_storage(
        Box::new(Generated::new(600_000)),
        scan::Options {
            memory_mode: crate::memory::BudgetMode::Manual { mib: 1024 },
            ..Default::default()
        },
    ));
    let mut times = Vec::new();
    let mut result_frame = Duration::ZERO;
    let started = Instant::now();
    while app.job.is_some() {
        let frame_start = Instant::now();
        frame(&mut app, &context, vec![]);
        let elapsed = frame_start.elapsed();
        if app.job.is_some() {
            times.push(elapsed);
        } else {
            result_frame = elapsed;
        }
        assert!(started.elapsed() < Duration::from_secs(60));
        if app.job.is_some() {
            std::thread::sleep(Duration::from_millis(16));
        }
    }
    let result = app.result.as_ref().unwrap();
    assert_eq!(result.nodes.len(), 600_001);
    assert!(!result.limited);
    times.sort();
    eprintln!(
        "scan responsiveness: objects={}, scan_ms={}, scanning_frames={}, frame_p95_us={}, frame_max_us={}, first_result_frame_us={}, estimated_scanner_bytes={}, budget_bytes={} (headless egui, release={})",
        result.nodes.len(),
        result.elapsed_millis,
        times.len(),
        times[times.len() * 95 / 100].as_micros(),
        times.last().unwrap().as_micros(),
        result_frame.as_micros(),
        result.memory.as_ref().unwrap().estimated_scanner_bytes,
        result.memory.as_ref().unwrap().budget_bytes,
        !cfg!(debug_assertions)
    );
    // Cancellation uses a new scan, without retaining/copying the previous tree.
    app.result = None;
    app.live = None;
    let job = app.service.start_storage(
        Box::new(Generated::new(usize::MAX)),
        scan::Options {
            memory_mode: crate::memory::BudgetMode::Manual { mib: 1024 },
            ..Default::default()
        },
    );
    while job.progress.lock().unwrap().objects < 100_000 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let cancel_start = Instant::now();
    job.cancel();
    let cancelled = loop {
        if let Some(result) = job.take() {
            break result;
        }
        assert!(cancel_start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(cancelled.cancelled);
    eprintln!(
        "cancel responsiveness: objects={}, cancel_to_result_us={} (includes aggregation and 1 ms polling)",
        cancelled.nodes.len(),
        cancel_start.elapsed().as_micros()
    );
}
