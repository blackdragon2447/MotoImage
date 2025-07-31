use byteorder::{BigEndian, LittleEndian, ReadBytesExt, WriteBytesExt};
use image::{GenericImageView, ImageReader, Pixel, Rgb, RgbImage};
use std::{
    ffi::CString,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use clap::{Args, Parser};

#[derive(Parser, Debug)]
enum Subcommands {
    Decode(DecodeArgs),
    Encode(EncodeArgs),
}

#[derive(Args, Debug)]
pub struct DecodeArgs {
    images_out: PathBuf,
    file_in: PathBuf,
}

#[derive(Args, Debug)]
pub struct EncodeArgs {
    images_in: PathBuf,
    file_out: PathBuf,
}

fn main() -> Result<(), io::Error> {
    let args = Subcommands::parse();

    match args {
        Subcommands::Decode(DecodeArgs {
            images_out,
            file_in,
        }) => {
            let images = unpack_file(file_in)?;

            println!("Finished loading\n");

            for (name, image) in &images {
                let mut img_path = images_out.clone();
                img_path.set_file_name(name);
                img_path.set_extension("png");
                println!("Saving {}", img_path.display());
                image.save(img_path).unwrap();
            }
        }
        Subcommands::Encode(EncodeArgs {
            images_in,
            file_out,
        }) => {
            println!("{}", images_in.display());
            let images = std::fs::read_dir(images_in)?
                .map(|d| -> Result<Option<(String, RgbImage)>, io::Error> {
                    let d = d?;
                    if d.file_type()?.is_file() {
                        let name = d.file_name().to_string_lossy().into_owned();
                        let Some(name) = name.strip_suffix(".png").map(str::to_string) else {
                            return Ok(None);
                        };
                        println!("Loading {}", name);
                        let image = image::open(d.path()).unwrap();
                        let image = image.to_rgb8();
                        Ok(Some((name, image)))
                    } else {
                        Ok(None)
                    }
                })
                .filter(|r| r.as_ref().is_ok_and(|o| o.is_some()) || r.is_err())
                .map(|r| r.map(Option::unwrap))
                .collect::<Result<Vec<_>, io::Error>>()?;

            let file_data = pack_file(images)?;
            std::fs::write(file_out, &file_data)?;
        }
    }

    Ok(())
}

fn align_up(val: usize, align: usize) -> usize {
    debug_assert!(align.is_power_of_two());
    (val + align - 1) & !(align - 1)
}

fn pack_file(images: Vec<(String, RgbImage)>) -> Result<Vec<u8>, io::Error> {
    let mut data = Vec::new();

    data.write_u64::<LittleEndian>(0x6F676F4C6F746F4D)?;
    data.write_u32::<LittleEndian>(((images.len() as u32 * 0x20) + 0x0D) << 8)?;
    data.write_u8(0)?;
    println!("{:#X}", ((images.len() as u32 * 0x20) + 0x0D) << 8);
    dbg!(images.len());

    let images = images
        .into_iter()
        .map(|(n, i)| -> Result<(String, Vec<u8>), io::Error> {
            let data = write_image(&i)?;
            Ok((n, data))
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut offset = align_up((images.len() * 0x20) + 0x0D, 0x100);

    for (name, image) in &images {
        println!("Processing \"{name}\"");
        let cstr = CString::from_str(name).unwrap();
        let bytes = cstr.to_bytes_with_nul();
        assert!(bytes.len() <= 0x18);
        for i in 0..0x18 {
            data.write_u8(*bytes.get(i).unwrap_or(&0))?;
        }
        data.write_i32::<LittleEndian>(offset as i32)?;
        data.write_i32::<LittleEndian>(image.len() as i32)?;
        println!("{:#X}", offset);

        offset += image.len();
        offset = align_up(offset, 0x100);
    }

    dbg!(offset, data.len());

    let mut offset = align_up((images.len() * 0x20) + 0x0D, 0x100);
    pad(&mut data, offset);

    for (name, image) in &images {
        println!("Writing \"{name}\"");
        dbg!(image.len());
        println!("{:#X}, {}, {}", offset, offset, data.len());
        data.write_all(&image)?;
        offset += image.len();
        offset = align_up(offset, 0x100);
        pad(&mut data, offset);
    }

    Ok(data)
}

fn pad(data: &mut Vec<u8>, new_len: usize) {
    let new_data = new_len - data.len();
    data.append(&mut vec![0; new_data]);
    assert_eq!(data.len(), new_len);
}

fn write_image(image: &RgbImage) -> Result<Vec<u8>, io::Error> {
    let mut data = Vec::new();
    data.write_u64::<LittleEndian>(0x006E75526F746F4D)?;
    data.write_u16::<BigEndian>(image.width() as u16)?;
    data.write_u16::<BigEndian>(image.height() as u16)?;
    for r in image.rows() {
        let mut iter = r.peekable();

        while let Some(px) = iter.next() {
            if iter.peek().is_some_and(|p| *p == px) {
                let mut count = 1;
                while let Some(_) = iter.next_if(|p| *p == px) {
                    count += 1;
                }
                if count < 0x1000 {
                    let count = count & 0xFFF;
                    data.write_u8(((0x8 << 4) | (count >> 8)) as u8)?;
                    data.write_u8((count & 0xFF) as u8)?;
                    data.write_u8(px.0[2])?;
                    data.write_u8(px.0[1])?;
                    data.write_u8(px.0[0])?;
                } else {
                    while count >= 0x1000 {
                        let sub_count = 0xFFF;
                        data.write_u8(((0x8 << 4) | (sub_count >> 8)) as u8)?;
                        data.write_u8((sub_count & 0xFF) as u8)?;
                        data.write_u8(px.0[2])?;
                        data.write_u8(px.0[1])?;
                        data.write_u8(px.0[0])?;
                        count -= 0xFFF;
                    }
                    let count = count & 0xFFF;
                    data.write_u8(((0x8 << 4) | (count >> 8)) as u8)?;
                    data.write_u8((count & 0xFF) as u8)?;
                    data.write_u8(px.0[2])?;
                    data.write_u8(px.0[1])?;
                    data.write_u8(px.0[0])?;
                }
            } else {
                let mut pixels = vec![px];
                while let Some(px) = iter.next_if(|p| p != pixels.last().unwrap()) {
                    pixels.push(px);
                }
                let count = pixels.len();
                if count < 0x1000 {
                    let count = count & 0xFFF;
                    data.write_u8(((0x0 << 4) | (count >> 8)) as u8)?;
                    data.write_u8((count & 0xFF) as u8)?;

                    for px in pixels {
                        data.write_u8(px.0[2])?;
                        data.write_u8(px.0[1])?;
                        data.write_u8(px.0[0])?;
                    }
                } else {
                    todo!()
                }
            }
        }
    }

    Ok(data)
}

fn unpack_file<P: AsRef<Path>>(file: P) -> Result<Vec<(String, RgbImage)>, io::Error> {
    let data = std::fs::read(file)?;
    dbg!(data.len());
    let mut reader: &[u8] = &data;

    assert_eq!(reader.read_u64::<LittleEndian>()?, 0x6F676F4C6F746F4D);

    let count_dat = reader.read_i32::<LittleEndian>()? >> 8;
    println!("{:#X}", count_dat);
    let count = ((count_dat - 0x0D) / 0x20) as usize;
    dbg!(count);

    let mut names: Vec<String> = Vec::with_capacity(count);
    let mut offsets: Vec<i32> = Vec::with_capacity(count);
    let mut sizes: Vec<i32> = Vec::with_capacity(count);

    for i in 0..count {
        let mut reader = &data[0x0D + (0x20 * i)..];
        let mut buf: Vec<u8> = Vec::from([0; 0x18]);

        reader.read_exact(&mut buf[0..0x18])?;
        let buf: Vec<u8> = buf.into_iter().take_while(|&b| b != 0).collect();
        let name = String::from_utf8(buf).unwrap();
        names.push(name);
        offsets.push(reader.read_i32::<LittleEndian>()?);
        sizes.push(reader.read_i32::<LittleEndian>()?);
    }

    let mut images = Vec::new();
    for i in 0..count {
        println!("Processing \"{}\"", names[i]);
        println!("offset: {:#X}", offsets[i]);
        let reader = &data[((offsets[i]) as usize)..];

        let image = read_image(reader)?;

        images.push((names[i].clone(), image));
    }

    println!("{:#X}", offsets.last().unwrap() + sizes.last().unwrap());

    Ok(images)
}

fn read_image(mut reader: &[u8]) -> Result<RgbImage, io::Error> {
    assert_eq!(reader.read_u64::<LittleEndian>()?, 0x006E75526F746F4D);
    let width = reader.read_u16::<BigEndian>()? as u32;
    let height = reader.read_u16::<BigEndian>()? as u32;
    dbg!(width, height);

    let mut image = RgbImage::new(width as u32, height as u32);

    let mut idx = 0;

    while idx < width * height {
        let mode_len_l = reader.read_u8()? as u16;
        let mode_len_h = reader.read_u8()? as u16;
        // let mode_len = reader.read_u16::<LittleEndian>()?;
        // println!("{:#04X}", mode_len_l);
        // println!("{:#04X}", mode_len_h);
        let mode = mode_len_l >> 4;
        let len = (((mode_len_l & 0xF) << 8) | mode_len_h) as u32;
        // println!("mode {:#03X}", mode);
        // println!("len {:#05X}", len);
        match mode {
            0x0 => {
                for i in 0..len {
                    let b = reader.read_u8()?;
                    let g = reader.read_u8()?;
                    let r = reader.read_u8()?;

                    image.put_pixel((idx + i) % width, idx / width, Rgb([r, g, b]));
                }
            }
            0x8 => {
                let b = reader.read_u8()?;
                let g = reader.read_u8()?;
                let r = reader.read_u8()?;

                for i in 0..len {
                    image.put_pixel((idx + i) % width, idx / width, Rgb([r, g, b]));
                }
            }
            _ => {
                dbg!(idx, width * height);
                panic!("{mode_len_l:#04X}{mode_len_h:02X}\n{mode:#X}");
            }
        }
        idx += len;
    }

    Ok(image)
}
