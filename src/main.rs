use std::error::Error;
use std::env::args;
use std::fs;
use bytes::{ Bytes, Buf };
use image::{ Rgba, RgbaImage, GenericImage };

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
	num_palettes: usize,
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

	for (i, image_offset) in image_offsets.iter().enumerate() {
		let image_buffer = Bytes::copy_from_slice(&data[*image_offset as usize..]);
		let image_def = read_image_def(image_buffer);

		// calc data offsets
		let start_index = *image_offset as usize;
		let palette_data_index = start_index + image_def.palette_data_offset;
		let pixel_data_index = start_index + image_def.pixel_data_offset;
		let end_index = start_index + image_def.data_length;

		println!("\nImage {}", i);
		println!("    is_encrypted: {:?}", image_def.is_encrypted);
		println!("    compression: {:?}", image_def.compression);
		println!("    num_palettes: {}", image_def.num_palettes);
		println!("    num_sprites: {}", image_def.num_sprites);
		println!("    sprite_width_px: {}", image_def.sprite_width_px);
		println!("    sprite_height_px: {}", image_def.sprite_height_px);
		println!("    image_width: {}", image_def.image_width);
		println!("    image_height: {}", image_def.image_height);

		// get color palettes
		let mut palettes = Vec::new();
		if let PixelDataType::Bpp(bpp) = image_def.pixel_data_type {
			let palette_data = &data[palette_data_index..pixel_data_index];
			let colors_per_palette = 2usize.pow(bpp as u32);
			palettes = get_palettes(palette_data, colors_per_palette, image_def.num_palettes);
		}

		// get pixel data for each sprite
		let pixel_data_per_sprite = get_pixel_data_per_sprite(&data[pixel_data_index..end_index], &image_def);

		// combine sprites into subimages, and subimages into a spritesheet, one row per palette
		let spritesheet = make_spritesheet(&image_def, &pixel_data_per_sprite, &palettes);

		// save spritesheet
		spritesheet.save(format!("{}image-{}.png", output_path, i)).expect("failed to save");
	}

	Ok(())
}

fn read_image_def(mut bytes: Bytes) -> ImageDef {
	let data_length = bytes.get_u32_le() as usize;

	// read flags
	let flags = bytes.get_u8();
	let has_transparency = (flags & 0b00000100) > 0;
	let compression = if (flags & 0b00100000) > 0 {
		CompressionType::Bytewise
	} else if (flags & 0b01000000) > 0 {
		CompressionType::Wordwise
	} else {
		CompressionType::None
	};
	let is_encrypted = (flags & 0b10000000) > 0;

	// determine bpp
	let pixel_data_type = match bytes.get_u8() {
		0 => PixelDataType::Bpp(1),
		1 => PixelDataType::Bpp(2),
		2 => PixelDataType::Bpp(4),
		3 => PixelDataType::Bpp(8),
		_ => PixelDataType::Direct
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
	let num_palettes = bytes.get_u8() as usize;
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
		num_palettes,
		transparent_color_index,
		palette_data_offset,
		pixel_data_offset
	}
}

fn parse_rgb565(value: u16) -> Rgba<u8> {
	let r = (value >> 11) * 255 / 31;
	let g = ((value >> 5) & 0b111111) * 255 / 63;
	let b = (value & 0b11111) * 255 / 31;
	Rgba([r as u8, g as u8, b as u8, 255])
}

fn get_palettes(bytes: &[u8], colors_per_palette: usize, num_palettes: usize) -> Vec<Vec<Rgba<u8>>> {
	let mut buf = Bytes::copy_from_slice(bytes);
	let mut palettes = vec![Vec::new(); num_palettes];

	// get all colors
	let mut colors = Vec::new();
	while buf.remaining() >= 2 {
		let value = buf.get_u16_le();
		let color = parse_rgb565(value);
		colors.push(color);
	}

	// assign colors to palettes
	for (i, color) in colors.iter().enumerate() {
		let palette_index = i / colors_per_palette;
		if palette_index < palettes.len() {
			palettes[palette_index].push(color.clone());
		}
	}

	palettes
}

fn get_pixel_data_per_sprite(data: &[u8], def: &ImageDef) -> Vec<Vec<u8>> {
	if let CompressionType::None = def.compression {
		get_uncompressed_pixel_data(data, def)
	} else {
		get_compressed_pixel_data(data, def)
	}
}

fn get_uncompressed_pixel_data(data: &[u8], def: &ImageDef) -> Vec<Vec<u8>> {
	// if uncompressed, each sprite has a fixed size
	let bytes_per_sprite = if let PixelDataType::Bpp(bpp) = def.pixel_data_type {
		let bits_per_sprite = def.sprite_width_px * def.sprite_height_px * bpp;
		if bits_per_sprite % 8 == 0 {
			bits_per_sprite / 8
		} else {
			bits_per_sprite / 8 + 1
		}
	} else {
		def.sprite_width_px * def.sprite_height_px * 2
	};

	let mut pixel_data_per_sprite = Vec::new();
	for j in 0..def.num_sprites {
		let a = bytes_per_sprite * j;
		let b = a + bytes_per_sprite;
		let pixel_data = if def.is_encrypted {
			decrypt_pixel_data(&data[a..b])
		} else {
			data[a..b].to_vec()
		};
		pixel_data_per_sprite.push(pixel_data);
	}
	pixel_data_per_sprite
}

fn get_compressed_pixel_data(data: &[u8], def: &ImageDef) -> Vec<Vec<u8>> {
	// if compressed, get offsets + lengths and use those to get pixel data per sprite
	let mut pixel_data_per_sprite = Vec::new();
	let mut buf = Bytes::copy_from_slice(data);
	for _ in 0..def.num_sprites {
		let a = buf.get_u32_le() as usize;
		let len = buf.get_u32_le() as usize;
		let pixel_data = if def.is_encrypted {
			decrypt_pixel_data(&data[a..(a+len)])
		} else {
			data[a..(a+len)].to_vec()
		};
		pixel_data_per_sprite.push(pixel_data);
	}
	pixel_data_per_sprite
}

fn decrypt_pixel_data(data: &[u8]) -> Vec<u8> {
	data.iter().map(|byte| byte ^ 0x53).collect()
}

fn decompress_bytewise(bytes: &[u8]) -> Vec<u8> {
	let mut chunks = Vec::new();
	let mut buf = Bytes::copy_from_slice(bytes);
	while buf.remaining() >= 1 {
		let control = buf.get_u8();
		let top_bit = control >> 7;
		let n = control & 0x7f;
		if top_bit == 1 && buf.remaining() >= n as usize {
			for _ in 0..n {
				let value = buf.get_u8();
				chunks.push(value);
			}
		} else if top_bit == 0 && buf.remaining() >= 1 {
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

fn make_sprite(data: &[u8], def: &ImageDef, palette: &[Rgba<u8>]) -> RgbaImage {
	// decompress pixel data
	let pixel_data = match def.compression {
		CompressionType::None => data.to_vec(),
		CompressionType::Bytewise => decompress_bytewise(&data),
		CompressionType::Wordwise => decompress_wordwise(&data)
	};

	// convert pixel data to images
	let sprite = if let PixelDataType::Bpp(bpp) = def.pixel_data_type {
		make_indexed_sprite(&pixel_data, &def, bpp, &palette)
	} else {
		make_direct_sprite(&pixel_data, &def)
	};

	sprite
}

fn make_indexed_sprite(bytes: &[u8], def: &ImageDef, bpp: usize, palette: &[Rgba<u8>]) -> RgbaImage {
	let mut img = RgbaImage::new(def.sprite_width_px as u32, def.sprite_height_px as u32);
	let mut buf = Bytes::copy_from_slice(bytes);

	// add bits to end of stream in least-significant order
	let mut bits = Vec::new();
	while buf.remaining() >= 1 {
		bits.extend(byte_to_bits(buf.get_u8()));
	}

	// divide bits into chunks of n bits, where n is bpp (bits per pixel)
	let chunks = bits.chunks(bpp);
	let expected_chunks = def.sprite_width_px * def.sprite_height_px;
	if chunks.len() != expected_chunks {
		println!("WARNING: expected {} chunks, got {}", expected_chunks, chunks.len());
	}

	// convert each chunk into a palette index and draw pixel
	for (i, chunk) in chunks.enumerate() {
		let x = i % def.sprite_width_px;
		let y = i / def.sprite_width_px;
		let index = bits_to_byte(chunk) as usize;
		let color = if def.has_transparency && index == def.transparent_color_index as usize {
			Rgba([0, 0, 0, 0])
		} else {
			palette.get(index).expect("color index is out of range for given palette").clone()
		};
		if x < def.sprite_width_px && y < def.sprite_height_px {
			img.put_pixel(x as u32, y as u32, color);
		}
	}

	img
}

fn make_direct_sprite(bytes: &[u8], def: &ImageDef) -> RgbaImage {
	let mut img = RgbaImage::new(def.sprite_width_px as u32, def.sprite_height_px as u32);
	let mut buf = Bytes::copy_from_slice(bytes);
	let mut i = 0;
	while bytes.remaining() >= 2 {
		let x = i % def.sprite_width_px;
		let y = i / def.sprite_width_px;
		let value = buf.get_u16_le();
		let mut color = parse_rgb565(value);
		if def.has_transparency && def.transparent_color_index == value {
			color = Rgba([0, 0, 0, 0]);
		}
		img.put_pixel(x as u32, y as u32, color);
		i += 1;
	}
	img
}

fn make_subimage(sprites: &[RgbaImage], def: &ImageDef) -> RgbaImage {
	let width = def.sprite_width_px * def.image_width;
	let height = def.sprite_height_px * def.image_height;
	let mut img = RgbaImage::new(width as u32, height as u32);
	for (i, sprite) in sprites.iter().enumerate() {
		let x = (i % def.image_width) * def.sprite_width_px;
		let y = (i / def.image_width) * def.sprite_height_px;
		img.copy_from(sprite, x as u32, y as u32).expect("unable to copy sprite into subimage");
	}
	img
}

fn make_spritesheet(def: &ImageDef, pixel_data_per_sprite: &[Vec<u8>], palettes: &[Vec<Rgba<u8>>]) -> RgbaImage {
	let sprites_per_subimage = def.image_width * def.image_height;
	let spritesheet_width = def.num_subimages * def.image_width * def.sprite_width_px;
	let spritesheet_height = def.num_palettes * def.image_height * def.sprite_height_px;
	let mut img = RgbaImage::new(spritesheet_width as u32, spritesheet_height as u32);
	for (i, palette) in palettes.iter().enumerate() {
		let sprites: Vec<RgbaImage> = pixel_data_per_sprite.iter().map(|pixel_data|
			make_sprite(pixel_data, def, palette)
		).collect();
		let subimages: Vec<RgbaImage> = (0..def.num_subimages).map(|j| {
			let a = j * sprites_per_subimage;
			let b = a + sprites_per_subimage;
			make_subimage(&sprites[a..b], def)
		}).collect();
		for (j, subimage) in subimages.iter().enumerate() {
			let x = j * def.image_width * def.sprite_width_px;
			let y = i * def.image_height * def.sprite_height_px;
			img.copy_from(subimage, x as u32, y as u32).expect("unable to copy subimage into spritesheet");
		}
	}
	img
}
