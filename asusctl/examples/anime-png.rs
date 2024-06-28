use std::convert::TryFrom;
use std::env;
use std::error::Error;
use std::path::Path;
use std::process::exit;

use rog_anime::usb::get_maybe_anime_type;
use rog_anime::{AnimeDataBuffer, AnimeImage, Vec2};
use rog_dbus::zbus_anime::AnimeProxyBlocking;
use zbus::blocking::Connection;

fn main() -> Result<(), Box<dyn Error>> {
    let conn = Connection::system().unwrap();
    let proxy = AnimeProxyBlocking::new(&conn).unwrap();

    let args: Vec<String> = env::args().collect();
    if args.len() != 7 {
        println!("Usage: <filepath> <scale> <angle> <x pos> <y pos> <brightness>");
        println!("e.g, asusctl/examples/doom_large.png 0.9 0.4 0.0 0.0 0.8");
        exit(-1);
    }

    let anime_type = get_maybe_anime_type()?;
    let matrix = AnimeImage::from_png(
        Path::new(&args[1]),
        args[2].parse::<f32>().unwrap(),
        args[3].parse::<f32>().unwrap(),
        Vec2::new(
            args[4].parse::<f32>().unwrap(),
            args[5].parse::<f32>().unwrap(),
        ),
        args[6].parse::<f32>().unwrap(),
        anime_type,
    )?;

    proxy.write(<AnimeDataBuffer>::try_from(&matrix)?).unwrap();

    Ok(())
}
