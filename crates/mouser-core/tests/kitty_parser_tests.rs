use mouser_core::kitty::{KittyGraphicsEvent, parse_kitty_graphics_params};

#[test]
fn parse_graphics_begin_and_delete() {
    let ev = parse_kitty_graphics_params("a=T,i=42,s=640,v=480");
    match ev {
        KittyGraphicsEvent::Begin { id, width, height } => {
            assert_eq!(id, Some(42));
            assert_eq!(width, Some(640));
            assert_eq!(height, Some(480));
        }
        _ => panic!("unexpected"),
    }
    let ev = parse_kitty_graphics_params("t=d,i=7");
    match ev {
        KittyGraphicsEvent::Delete { id } => assert_eq!(id, Some(7)),
        _ => panic!("unexpected"),
    }
}
