use std::error::Error;
use std::env::args;
use std::fs;
use bytes::{ Bytes, Buf };
use image::{ Rgba, RgbaImage };

// Format: https://gist.github.com/GMMan/a467961057d1e9fb08a2bbfd553180d6

#[derive(Debug)]
enum CompressionType {
	None,
	Bytewise,
	Wordwise
}

#[derive(Debug)]
enum PixelDataType {
	Bpp(usize),
	Direct
}

struct ImageDef {
	data_length: usize,
	has_transparency: bool,
	is_encrypted: bool,
	compression: CompressionType,
	pixel_data_type: PixelDataType,
	num_sprites: usize,
	sprite_width_px: usize,
	sprite_height_px: usize,
	offset_x: i8,
	offset_y: i8,
	image_width: usize,
	image_height: usize,
	num_palette_sets: usize,
	transparent_color_index: u16,
	palette_data_offset: usize,
	pixel_data_offset: usize,
	num_subimages: usize
}

fn main() -> Result<(), Box<dyn Error + 'static>> {
	let input_path = args().nth(1).expect("no input path given");
	let mut output_path = args().nth(2).expect("no output path given");
	if !output_path.ends_with('/') {
		output_path = format!("{}/", output_path);
	}

	let data = fs::read(input_path)?;
	let mut buffer = Bytes::copy_from_slice(&data);

	// get image offsets
	let first_image_offset = buffer.get_u32_le();
	let mut image_offsets: Vec<u32> = vec![first_image_offset];
	let mut current_offset = 4;
	while current_offset < first_image_offset {
		let image_offset = buffer.get_u32_le();
		image_offsets.push(image_offset);
		current_offset += 4;
	}
	println!("Image Count: {}", image_offsets.len());

	for (i, image_offset) in image_offsets.iter().enumerate() {
		let image_buffer = Bytes::copy_from_slice(&data[*image_offset as usize..]);
		let image_def = read_image_def(image_buffer);

		// calc data offsets
		let start_index = *image_offset as usize;
		let palette_data_index = start_index + image_def.palette_data_offset;
		let pixel_data_index = start_index + image_def.pixel_data_offset;
		let end_index = start_index + image_def.data_length;

		if i < 10 {
			println!("\nImage {}", i);
			println!("    encrypted: {:?}", image_def.is_encrypted);
			println!("    compressed: {:?}", image_def.compression);
			println!("    pixel_data_type: {:?}", image_def.pixel_data_type);
			println!("    num_palette_sets: {}", image_def.num_palette_sets);
			println!("    num_sprites: {}", image_def.num_sprites);
			println!("    num_subimages: {}", image_def.num_subimages);
		}

		// get color palettes
		let mut palette_sets = Vec::new();
		if let PixelDataType::Bpp(bpp) = image_def.pixel_data_type {
			let palette_data = &data[palette_data_index..pixel_data_index];
			let colors_per_palette = 2usize.pow(bpp as u32);
			palette_sets = get_palette_sets(palette_data, colors_per_palette, image_def.num_palette_sets);
		}

		// decrypt pixel data
		let raw_pixel_data = if image_def.is_encrypted {
			decrypt_pixel_data(&data[pixel_data_index..end_index])
		} else {
			data[pixel_data_index..end_index].to_vec()
		};

		// get pixel data for each sprite
		let mut pixel_data_per_sprite = Vec::new();
		if let CompressionType::None = image_def.compression {
			// if uncompressed, just grab 2 bytes per pixel per sprite
			let bytes_per_sprite = image_def.sprite_width_px * image_def.sprite_height_px * 2;
			for j in 0..image_def.num_sprites {
				let offset = bytes_per_sprite * j;
				pixel_data_per_sprite.push(&raw_pixel_data[offset..(offset + bytes_per_sprite)]);
			}
		} else {
			// if compressed, get offsets + lengths and use those to get pixel data per sprite
			let mut buf = Bytes::copy_from_slice(&raw_pixel_data);
			for _ in 0..image_def.num_sprites {
				let offset = buf.get_u32_le() as usize;
				let length = buf.get_u32_le() as usize;
				println!("offset: {}, length: {}", offset, length);
				pixel_data_per_sprite.push(&raw_pixel_data[offset..(offset + length)]);
			}
		}

		// draw each sprite
		let mut sprites = Vec::new();
		for (j, raw_pixel_data_for_sprite) in pixel_data_per_sprite.iter().enumerate() {
			// decompress pixel data
			let pixel_data = match image_def.compression {
				CompressionType::None => raw_pixel_data_for_sprite.to_vec(),
				CompressionType::Bytewise => decompress_bytewise(&raw_pixel_data_for_sprite),
				CompressionType::Wordwise => decompress_wordwise(&raw_pixel_data_for_sprite)
			};

			// convert pixel data to images
			let sprite = if let PixelDataType::Bpp(bpp) = image_def.pixel_data_type {
				make_indexed_sprite(&pixel_data, &image_def, bpp, &palette_sets[0])
			} else {
				make_direct_sprite(&pixel_data, &image_def)
			};

			// TEMP: save each sprite
			let _ = sprite.save(format!("{}image{}-sprite{}.png", output_path, i, j));
			sprites.push(sprite);
		}

		// TODO: combine sprites into subimages
	}

	Ok(())
}

fn read_image_def(mut bytes: Bytes) -> ImageDef {
	let data_length = bytes.get_u32_le() as usize;

	// read flags
	let flags = bytes.get_u8();
	let has_transparency = (flags & 0b00100000) > 0;
	let compression = if (flags & 0b00000100) > 0 {
		CompressionType::Bytewise
	} else if (flags & 0b00000010) > 0 {
		CompressionType::Wordwise
	} else {
		CompressionType::None
	};
	let is_encrypted = (flags & 0b00000001) > 0;

	let bpp = bytes.get_u8() as usize;
	let pixel_data_type = if bpp < 16 {
		PixelDataType::Bpp(bpp)
	} else {
		PixelDataType::Direct
	};

	// read other properties
	let num_sprites = bytes.get_u16_le() as usize;
	let sprite_width_px = bytes.get_u8() as usize;
	let sprite_height_px = bytes.get_u8() as usize;
	let offset_x = bytes.get_i8();
	let offset_y = bytes.get_i8();
	let image_width = bytes.get_u8() as usize;
	let image_height = bytes.get_u8() as usize;
	let _unknown = bytes.get_u8(); // always 17
	let num_palette_sets = bytes.get_u8() as usize;
	let transparent_color_index = bytes.get_u16_le();
	let palette_data_offset = bytes.get_u16_le() as usize;
	let pixel_data_offset = bytes.get_u16_le() as usize;
	let _padding = bytes.get_u16_le(); // always 0

	// calc number of subimages
	let num_subimages = num_sprites / (image_width * image_height);

	// return image def
	ImageDef {
		data_length,
		has_transparency,
		is_encrypted,
		compression,
		pixel_data_type,
		num_sprites,
		num_subimages,
		sprite_width_px,
		sprite_height_px,
		offset_x,
		offset_y,
		image_width,
		image_height,
		num_palette_sets,
		transparent_color_index,
		palette_data_offset,
		pixel_data_offset
	}
}

fn get_palette_sets(bytes: &[u8], colors_per_palette: usize, num_palette_sets: usize) -> Vec<Vec<Rgba<u8>>> {
	let mut buf = Bytes::copy_from_slice(bytes);
	let mut palette_sets = vec![Vec::new(); num_palette_sets];

	// get all colors
	let mut colors = Vec::new();
	while buf.remaining() >= 2 {
		let value = buf.get_u16_le();
		let r = (value >> 8) as u8;
		let g = (value >> 3) as u8;
		let b = (value << 3) as u8;
		colors.push(Rgba([r, g, b, 255]));
	}

	// assign colors to palettes
	for (i, color) in colors.iter().enumerate() {
		let palette_set = i / colors_per_palette;
		if palette_set < palette_sets.len() {
			palette_sets[palette_set].push(color.clone());
		}
	}

	palette_sets
}

fn decrypt_pixel_data(data: &[u8]) -> Vec<u8> {
	println!("decrypting...");
	data.iter().map(|byte| byte ^ 0x53).collect()
}

fn decompress_bytewise(bytes: &[u8]) -> Vec<u8> {
	let mut chunks = Vec::new();
	let mut buf = Bytes::copy_from_slice(bytes);
	while buf.remaining() >= 1 {
		let control = buf.get_u8();
		let top_bit = control >> 7;
		let n = control & 0x7f;
		if top_bit == 1 {
			// add next n chunks
			for _ in 0..n {
				let value = buf.get_u8();
				chunks.push(value);
			}
		} else {
			// repeat [value] n times
			let value = buf.get_u8();
			for _ in 0..n {
				chunks.push(value);
			}
		}
	}
	chunks
}

fn decompress_wordwise(bytes: &[u8]) -> Vec<u8> {
	let mut chunks = Vec::new();
	let mut buf = Bytes::copy_from_slice(bytes);
	while buf.remaining() >= 1 {
		let control = buf.get_u32_le();
		let top_bit = control >> 31;
		let n = (control & 0x0fffffff) as usize;
		if top_bit > 0 {
			// add next n chunks
			for _ in 0..n {
				let value = buf.get_u32().to_le_bytes();
				chunks.extend(value.iter());
			}
		} else {
			// repeat [value] n times
			let value = buf.get_u32().to_le_bytes();
			for _ in 0..n {
				chunks.extend(value.iter());
			}
		}
	}
	chunks
}

fn byte_to_bits(byte: u8) -> Vec<u8> {
	let mut bits = Vec::new();
	for i in 0..8 {
		bits.push((byte >> i) & 1);
	}
	bits
}

fn bits_to_byte(bits: &[u8]) -> u8 {
	let mut byte = 0;
	for (i, bit) in bits.iter().enumerate() {
		byte = byte | (bit << i);
	}
	byte
}

fn make_indexed_sprite(bytes: &[u8], image_def: &ImageDef, bpp: usize, palette: &[Rgba<u8>]) -> RgbaImage {
	let mut img = RgbaImage::new(image_def.sprite_width_px as u32, image_def.sprite_height_px as u32);
	let mut buf = Bytes::copy_from_slice(bytes);

	// add bits to end of stream in least-significant order
	let mut bits = Vec::new();
	while bytes.remaining() >= 1 {
		bits.extend(byte_to_bits(buf.get_u8()));
	}

	// divide bits into chunks of n bits, where n is bpp (bits per pixel)
	let chunks = bits.chunks(bpp);

	// convert each chunk into a palette index and draw pixel
	for (i, chunk) in chunks.enumerate() {
		let x = i % image_def.sprite_width_px;
		let y = i / image_def.sprite_height_px;
		let index = bits_to_byte(chunk) as usize;
		let color = if image_def.has_transparency && index == image_def.transparent_color_index as usize {
			Rgba([0, 0, 0, 0])
		} else {
			palette.get(index).expect("color index is out of range for given palette").clone()
		};
		img.put_pixel(x as u32, y as u32, color);
	}

	img
}

fn make_direct_sprite(bytes: &[u8], image_def: &ImageDef) -> RgbaImage {
	let mut img = RgbaImage::new(image_def.sprite_width_px as u32, image_def.sprite_height_px as u32);
	let mut buf = Bytes::copy_from_slice(bytes);
	let mut i = 0;
	while bytes.remaining() >= 2 {
		let x = i % image_def.sprite_width_px;
		let y = i / image_def.sprite_height_px;
		let value = buf.get_u16_le();
		let r = (value >> 8) as u8;
		let g = (value >> 3) as u8;
		let b = (value << 3) as u8;
		let mut color = Rgba([r, g, b, 255]);
		if image_def.has_transparency && image_def.transparent_color_index == value {
			color = Rgba([0, 0, 0, 0]);
		}
		img.put_pixel(x as u32, y as u32, color);
		i += 1;
	}
	img
}
