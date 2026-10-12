// Copyright (C) 2004-2025 Artifex Software, Inc.
//
// This file is part of MuPDF.
//
// MuPDF is free software: you can redistribute it and/or modify it under the
// terms of the GNU Affero General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version.
//
// MuPDF is distributed in the hope that it will be useful, but WITHOUT ANY
// WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
// FOR A PARTICULAR PURPOSE. See the GNU Affero General Public License for more
// details.
//
// You should have received a copy of the GNU Affero General Public License
// along with MuPDF. If not, see <https://www.gnu.org/licenses/agpl-3.0.en.html>
//
// Alternative licensing terms are available from the licensor.
// For commercial licensing, see <https://www.artifex.com/> or contact
// Artifex Software, Inc., 39 Mesa Street, Suite 108A, San Francisco,
// CA 94129, USA, for further information.

/*
	Inkwell: a copy of MuPDF's pdf-image-rewriter.c with
	  - a per-page progress/cancel callback,
	  - per-image failure tolerance (an image that cannot be rewritten is kept as is),
	  - counters of what happened,
	  - the unique-image dpi index kept in step when an image is skipped (upstream drifts there),
	  - alpha pixmaps left alone (their sample layout does not match the rebuilt image), except imagemasks.
	Static helpers are private to this file; the public entry points are mupdf_pdf_rewrite_images_progress
	and mupdf_pdf_subset_all_fonts at the bottom.
*/

#include "internal.h"

#include <string.h>
#include <math.h>

/* Re-encode like the source (SAME), but only when the pixels were resampled; otherwise leave the image alone. */
#define INK_RECOMPRESS_SAME_IF_SCALED 6

typedef struct
{
	int images_seen;
	int images_resampled;
	int images_replaced;
	int images_kept;
	int images_failed;
} mupdf_rewrite_stats;

/* Returns non-zero to cancel. pass: 0 = scan, 1 = rewrite. */
typedef int (*mupdf_rewrite_progress)(void *opaque, int pass, int page, int pages);

typedef struct
{
	int num;
	int gen;
	float dpi;
} ink_image_details;

typedef struct
{
	int max;
	int len;
	int *uimg;
} ink_image_list;

typedef struct
{
	int max;
	int len;
	ink_image_details *img;
} ink_unique_image_list;

typedef struct
{
	ink_image_list list;
	ink_unique_image_list uilist;
	pdf_image_rewriter_options *opts;
	int which;
	mupdf_rewrite_stats *stats;
	mupdf_rewrite_progress progress;
	void *opaque;
	int pages;
	int cancelled;
} ink_image_info;

static float
ink_dpi_from_ctm(fz_matrix ctm, int w, int h)
{
	float expx = sqrtf(ctm.a * ctm.a + ctm.b * ctm.b);
	float expy = sqrtf(ctm.c * ctm.c + ctm.d * ctm.d);
	float dpix = w * 72.0f / (expx == 0 ? 1 : expx);
	float dpiy = h * 72.0f / (expy == 0 ? 1 : expy);

	if (dpix > dpiy)
		return dpiy;

	return dpix;
}

static void
ink_gather_image_rewrite(fz_context *ctx, void *opaque, fz_image **image, fz_matrix ctm, pdf_obj *im_obj)
{
	ink_image_info *info = (ink_image_info *)opaque;
	ink_image_list *ilist = &info->list;
	ink_unique_image_list *uilist = &info->uilist;
	int i, num, gen;
	float dpi;

	if (im_obj == NULL)
		return; /* Inline image, don't need to pregather that. */

	num = pdf_to_num(ctx, im_obj);
	gen = pdf_to_gen(ctx, im_obj);

	dpi = ink_dpi_from_ctm(ctm, (*image)->w, (*image)->h);

	/* Find this in the unique image list */
	for (i = 0; i < uilist->len; i++)
	{
		/* Found one already. Keep the smaller of the dpi's. */
		if (uilist->img[i].num == num &&
			uilist->img[i].gen == gen)
		{
			if (dpi < uilist->img[i].dpi)
				uilist->img[i].dpi = dpi;
			break;
		}
	}

	if (i == uilist->len)
	{
		/* Need to add a new unique image. */
		if (uilist->max == uilist->len)
		{
			int max2 = uilist->max * 2;
			if (max2 == 0)
				max2 = 32; /* Arbitrary */
			uilist->img = fz_realloc(ctx, uilist->img, max2 * sizeof(*uilist->img));
			uilist->max = max2;
		}

		uilist->img[uilist->len].num = num;
		uilist->img[uilist->len].gen = gen;
		uilist->img[uilist->len].dpi = dpi;
		uilist->len++;
	}

	/* Now we need to add an entry in the unique image list saying that entry n in
	 * that list corresponds to the ith unique image. */
	if (ilist->max == ilist->len)
	{
		int max2 = ilist->max * 2;
		if (max2 == 0)
			max2 = 32; /* Arbitrary */
		ilist->uimg = fz_realloc(ctx, ilist->uimg, max2 * sizeof(*ilist->uimg));
		ilist->max = max2;
	}

	ilist->uimg[ilist->len++] = i;
}

typedef enum
{
	ink_IMAGE_COLOR = 0,
	ink_IMAGE_GRAY,
	ink_IMAGE_BITONAL
} ink_image_type;

static ink_image_type
ink_classify_pixmap(fz_context *ctx, fz_pixmap *pix)
{
	/* For now, spots means color. In future we could check to
	 * see if all the spots were 0? */
	if (pix->s)
		return ink_IMAGE_COLOR;

	if (fz_colorspace_is_gray(ctx, pix->colorspace) || pix->colorspace == NULL)
	{
		int n = pix->n;
		int h = pix->h;
		ptrdiff_t span_step = pix->stride - pix->w * n;
		const unsigned char *s = pix->samples;

		/* Loop until we know it's not bitonal */
		if (pix->alpha && pix->n > 1)
		{
			while (h--)
			{
				int w = pix->w;

				while (w--)
				{
					if (s[1] == 0)
					{
						/* If alpha is zero, other components don't matter. */
					}
					else if (s[0] != 0 && s[0] != 255)
						return ink_IMAGE_GRAY;
					s += 2;
				}
				s += span_step;
			}
			return ink_IMAGE_BITONAL;
		}
		else
		{
			while (h--)
			{
				int w = pix->w;

				while (w--)
				{
					if (s[0] != 0 && s[0] != 255)
						return ink_IMAGE_GRAY;
					s++;
				}
				s += span_step;
			}
			return ink_IMAGE_BITONAL;
		}
	}
	else if (fz_colorspace_is_rgb(ctx, pix->colorspace))
	{
		int n = pix->n;
		int h = pix->h;
		int w;
		ptrdiff_t span_step = pix->stride - pix->w * n;
		const unsigned char *s = pix->samples;

		/* Is this safe, cos of profiles? */
		if (pix->alpha)
		{
			/* Loop until we know it's not bitonal */
			while (h--)
			{
				w = pix->w;
				while (w--)
				{
					if (s[3] == 0)
					{
						/* If alpha is zero, other components don't matter. */
					}
					else if (s[0] == s[1] && s[0] == s[2])
					{
						/* Plausibly gray */
						if (s[0] != 0 && s[0] != 255)
							goto rgba_not_bitonal; /* But not bitonal */
						if (s[3] != 0 && s[3] != 255)
							goto rgba_not_bitonal;
					}
					else
						return ink_IMAGE_COLOR;
					s += n;
				}
				s += span_step;
			}
			return ink_IMAGE_BITONAL;

			/* Loop until we know it's not gray */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[3] == 0)
					{
						/* If alpha is zero, other components don't matter. */
					}
					else if (s[0] != s[1] || s[0] != s[2])
						return ink_IMAGE_COLOR;
rgba_not_bitonal:
					s += n;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;
		}
		else
		{
			/* Loop until we know it's not bitonal */
			while (h--)
			{
				w = pix->w;
				while (w--)
				{
					if (s[0] == s[1] && s[0] == s[2])
					{
						if (s[0] != 0 && s[0] != 255)
							goto rgb_not_bitonal;
					}
					else
						return ink_IMAGE_COLOR;
					s += n;
				}
				s += span_step;
			}
			return ink_IMAGE_BITONAL;

			/* Loop until we know it's not gray */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[0] != s[1] || s[0] != s[2])
						return ink_IMAGE_COLOR;
rgb_not_bitonal:
					s += n;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;
		}
	}
	else if (fz_colorspace_is_cmyk(ctx, pix->colorspace))
	{
		int n = pix->n;
		int h = pix->h;
		int w;
		ptrdiff_t span_step = pix->stride - pix->w * n;
		const unsigned char *s = pix->samples;

		if (pix->alpha)
		{
			/* Loop until we know it's not bitonal */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[4] == 0)
					{
						/* If alpha is 0, other components don't matter. */
					}
					else if (s[0] == 0 && s[1] == 0 && s[2] == 0)
					{
						if (s[3] != 0 && s[3] != 255)
							goto cmyka_not_bitonal;
					}
					else
						return ink_IMAGE_COLOR;
					s += 5;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;

			/* Loop until we know it's not gray */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[4] == 0)
					{
						/* If alpha is 0, other components don't matter. */
					}
					else if (s[0] != 0 || s[1] != 0 || s[2] != 0)
						return ink_IMAGE_COLOR;
cmyka_not_bitonal:
					s += 5;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;
		}
		else
		{
			/* Loop until we know it's not bitonal */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[0] == 0 && s[1] == 0 && s[2] != 0)
					{
						if (s[3] != 0 && s[3] != 255)
							goto cmyk_not_bitonal;
					}
					else
						return ink_IMAGE_COLOR;
					s += 4;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;

			/* Loop until we know it's not gray */
			while (h--)
			{
				w = pix->w;

				while (w--)
				{
					if (s[0] != 0 || s[1] != 0 || s[2] != 0)
						return ink_IMAGE_COLOR;
cmyk_not_bitonal:
					s += 4;
				}
				s += span_step;
			}
			return ink_IMAGE_GRAY;
		}
	}
	return ink_IMAGE_COLOR;
}

static fz_pixmap *
ink_resample(fz_context *ctx, fz_pixmap *src, int method, float from_dpi, float to_dpi)
{
	int w2 = src->w;
	int h2 = src->h;
	int w = (int)(w2 * to_dpi / from_dpi + 0.5f);
	int h = (int)(h2 * to_dpi / from_dpi + 0.5f);
	int factor;

	/* Allow for us shrinking an image to 0.*/
	assert(w >= 0 && h >= 0);
	if (w == 0)
		w = 1;
	if (h == 0)
		h = 1;

	/* Allow for the possibility that we might only want to make such a tiny change
	 * in dpi that the image doesn't really resize. */
	if (w >= w2 && h >= h2)
		return NULL;

	if (method == FZ_SUBSAMPLE_BICUBIC)
	{
		fz_irect clip = { 0, 0, w, h };
		return fz_scale_pixmap(ctx, src, 0, 0, w, h, &clip);
	}

	factor = 0;
	while (1)
	{
		int w3 = (w2+1)/2;
		int h3 = (h2+1)/2;
		if (w3 <= w || h3 <= h)
			break;
		factor++;
		w2 = w3;
		h2 = h3;
	}

	fz_subsample_pixmap(ctx, src, factor);

	return fz_keep_pixmap(ctx, src);
}

static fz_compressed_buffer *
ink_fz_recompress_image_as_jpeg(fz_context *ctx, fz_pixmap *pix, const char *quality, fz_colorspace **cs)
{
	fz_compressed_buffer *cbuf = NULL;
	fz_pixmap *rgb = NULL;
	fz_output *out = NULL;
	int q = fz_atoi(quality);

	if (q == 0)
		q = 75; /* Default quality */

	if (!pix->colorspace)
		return NULL;

	if (!fz_colorspace_is_cmyk(ctx, pix->colorspace) &&
		!fz_colorspace_is_gray(ctx, pix->colorspace) &&
		!fz_colorspace_is_rgb(ctx, pix->colorspace))
	{
		/* We're going to need to convert colorspace. */
		/* It's not gray, so we need a color space - pick rgb. */
		pix = rgb = fz_convert_pixmap(ctx, pix, fz_device_rgb(ctx), NULL, NULL, fz_default_color_params, 0);
		*cs = fz_device_rgb(ctx);
	}

	fz_var(cbuf);
	fz_var(out);

	fz_try(ctx)
	{
		/* Write the samples as they are. fz_new_buffer_from_pixmap_as_jpeg converts anything that is not
		 * one of the three device spaces (an ICC gray, say) to RGB, which would not match the
		 * colorspace the new image is declared with. */
		cbuf = fz_new_compressed_buffer(ctx);
		cbuf->buffer = fz_new_buffer(ctx, 1024);
		out = fz_new_output_with_buffer(ctx, cbuf->buffer);
		fz_write_pixmap_as_jpeg(ctx, out, pix, q, 0);
		fz_close_output(ctx, out);
		cbuf->params.type = FZ_IMAGE_JPEG;
		cbuf->params.u.jpeg.color_transform = -2;
	}
	fz_always(ctx)
	{
		fz_drop_output(ctx, out);
		if (rgb)
			fz_drop_pixmap(ctx, rgb);
	}
	fz_catch(ctx)
	{
		fz_drop_compressed_buffer(ctx, cbuf);
		fz_rethrow(ctx);
	}

	return cbuf;
}

static fz_compressed_buffer *
ink_fz_recompress_image_as_j2k(fz_context *ctx, fz_pixmap *pix, const char *quality)
{
	fz_compressed_buffer *cbuf = fz_new_compressed_buffer(ctx);
	fz_output *out = NULL;
	int q = fz_atoi(quality);

	if (q <= 0)
		q = 80; /* Default 1:20 compression */
	if (q > 100)
		q = 100;

	fz_var(out);

	fz_try(ctx)
	{
		cbuf->buffer = fz_new_buffer(ctx, 1024);
		out = fz_new_output_with_buffer(ctx, cbuf->buffer);

		fz_write_pixmap_as_jpx(ctx, out, pix, q);
		cbuf->params.type = FZ_IMAGE_JPX;
		cbuf->params.u.jpx.smask_in_data = 0;
	}
	fz_always(ctx)
	{
		fz_drop_output(ctx, out);
	}
	fz_catch(ctx)
	{
		fz_drop_compressed_buffer(ctx, cbuf);
		fz_rethrow(ctx);
	}

	return cbuf;
}

static fz_compressed_buffer *
ink_fz_recompress_image_as_flate(fz_context *ctx, fz_pixmap *pix, const char *quality)
{
	fz_compressed_buffer *cbuf = fz_new_compressed_buffer(ctx);
	fz_output *out = NULL;
	fz_output *out2 = NULL;
	int h = pix->h;
	size_t n = (size_t) pix->w * pix->n;
	const unsigned char *samp = pix->samples;
	ptrdiff_t str = pix->stride;
	int q = fz_atoi(quality);

	/* Notionally, it's 0-100 */
	q /= 11;
	if (q > FZ_DEFLATE_BEST)
		q = FZ_DEFLATE_BEST;
	if (q <= 0)
		q = FZ_DEFLATE_DEFAULT;

	fz_var(out);
	fz_var(out2);

	fz_try(ctx)
	{
		cbuf->buffer = fz_new_buffer(ctx, 1024);
		out = fz_new_output_with_buffer(ctx, cbuf->buffer);
		out2 = fz_new_deflate_output(ctx, out, q, 0);

		while (h--)
		{
			fz_write_data(ctx, out2, samp, n);
			samp += str;
		}

		fz_close_output(ctx, out2);
		fz_drop_output(ctx, out2);
		out2 = NULL;
		fz_close_output(ctx, out);
	}
	fz_always(ctx)
	{
		fz_drop_output(ctx, out2);
		fz_drop_output(ctx, out);
	}
	fz_catch(ctx)
	{
		fz_drop_compressed_buffer(ctx, cbuf);
		fz_rethrow(ctx);
	}

	cbuf->params.type = FZ_IMAGE_FLATE;
	cbuf->params.u.flate.bpc = 8;
	cbuf->params.u.flate.colors = 0;
	cbuf->params.u.flate.predictor = 0;
	cbuf->params.u.flate.columns = 0;

	return cbuf;
}

static fz_compressed_buffer *
ink_fz_recompress_image_as_fax(fz_context *ctx, fz_pixmap *pix)
{
	/* FIXME: Should get default colorspaces from the doc! */
	fz_default_colorspaces *defcs = fz_new_default_colorspaces(ctx);
	fz_compressed_buffer *cbuf = NULL;
	fz_halftone *ht = NULL;
	fz_bitmap *bmp = NULL;
	fz_buffer *inv_buffer = NULL;

	fz_var(ht);
	fz_var(bmp);
	fz_var(cbuf);
	fz_var(inv_buffer);
	/* pix is replaced inside the try block and dropped in the always block. */
	fz_var(pix);

	fz_keep_pixmap(ctx, pix);
	fz_try(ctx)
	{

		/* Convert to alphaless grey */
		if (pix->n != 1)
		{
			fz_pixmap *pix2 = fz_convert_pixmap(ctx, pix, fz_device_gray(ctx), NULL, defcs, fz_default_color_params, 0);

			fz_drop_pixmap(ctx, pix);
			pix = pix2;
		}

		/* Convert to a bitmap */
		ht = fz_default_halftone(ctx, 1);

		bmp = fz_new_bitmap_from_pixmap(ctx, pix, ht);

		cbuf = fz_new_compressed_buffer(ctx);
		cbuf->buffer = fz_compress_ccitt_fax_g4(ctx, bmp->samples, bmp->w, bmp->h, bmp->stride);
		cbuf->params.type = FZ_IMAGE_FAX;
		cbuf->params.u.fax.k = -1;
		cbuf->params.u.fax.columns = pix->w;
		cbuf->params.u.fax.rows = pix->h;

		fz_invert_bitmap(ctx, bmp);
		inv_buffer = fz_compress_ccitt_fax_g4(ctx, bmp->samples, bmp->w, bmp->h, bmp->stride);

		/* cbuf->buffer requires "/BlackIs1 true ", so it needs to beats the inverted one by
		 * at least 15 bytes, or we'll use the inverted one. */
		if (cbuf->buffer->len + 15 < inv_buffer->len)
		{
			cbuf->params.u.fax.black_is_1 = 1;
		}
		else
		{
			fz_drop_buffer(ctx, cbuf->buffer);
			cbuf->buffer = inv_buffer;
			inv_buffer = NULL;
		}
	}
	fz_always(ctx)
	{
		fz_drop_bitmap(ctx, bmp);
		fz_drop_halftone(ctx, ht);
		fz_drop_pixmap(ctx, pix);
		fz_drop_buffer(ctx, inv_buffer);
		fz_drop_default_colorspaces(ctx, defcs);
	}
	fz_catch(ctx)
	{
		fz_drop_compressed_buffer(ctx, cbuf);
		fz_rethrow(ctx);
	}

	return cbuf;
}

static int ink_method_from_fmt(int fmt)
{
	switch (fmt)
	{
	case FZ_IMAGE_JPEG:
		return FZ_RECOMPRESS_JPEG;
	case FZ_IMAGE_JPX:
		return FZ_RECOMPRESS_J2K;
	case FZ_IMAGE_FAX:
		return FZ_RECOMPRESS_FAX;
	}
	return FZ_RECOMPRESS_LOSSLESS;
}

static fz_image *
ink_recompress_image(fz_context *ctx, fz_pixmap *pix, int type, int fmt, int method, const char *quality, fz_image *oldimg, int scaled)
{
	int interpolate = oldimg->interpolate;
	fz_compressed_buffer *cbuf = NULL;
	fz_colorspace *cs = pix->colorspace;
	int bpc = 8;

	(void)type;
	if (method == INK_RECOMPRESS_SAME_IF_SCALED)
		method = scaled ? FZ_RECOMPRESS_SAME : FZ_RECOMPRESS_NEVER;

	if (method == FZ_RECOMPRESS_NEVER)
		return NULL;

	if (method == FZ_RECOMPRESS_SAME)
		method = ink_method_from_fmt(fmt);

	/* MuPDF's JPX encoder goes lossless when the image is smaller than two 256 px tiles in both
	 * directions, which is far bigger than a JPEG. Use JPEG for those, at the same quality number. */
	if (method == FZ_RECOMPRESS_J2K && pix->w < 512 && pix->h < 512)
		method = FZ_RECOMPRESS_JPEG;

	if (method == FZ_RECOMPRESS_J2K)
		cbuf = ink_fz_recompress_image_as_j2k(ctx, pix, quality);
	if (method == FZ_RECOMPRESS_JPEG)
		cbuf = ink_fz_recompress_image_as_jpeg(ctx, pix, quality, &cs);
	if (method == FZ_RECOMPRESS_FAX)
	{
		cbuf = ink_fz_recompress_image_as_fax(ctx, pix);
		if (cbuf)
		{
			bpc = 1;
			if (!oldimg->imagemask)
				cs = fz_device_gray(ctx);
		}
	}
	if (cbuf == NULL)
		cbuf = ink_fz_recompress_image_as_flate(ctx, pix, quality);

	if (cbuf == NULL)
		return NULL;

	/* fz_new_image_from_compressed_buffer takes ownership of compressed buffer, even
	 * in failure case. */
	return fz_new_image_from_compressed_buffer(ctx, pix->w, pix->h, bpc, cs, pix->xres, pix->yres, interpolate, oldimg->imagemask, NULL, NULL, cbuf, oldimg->mask);
}

static void
ink_do_image_rewrite_inner(fz_context *ctx, ink_image_info *info, fz_image **image, pdf_obj *im_obj, float dpi)
{
	fz_pixmap *pix;
	fz_pixmap *newpix = NULL;
	ink_image_type type;
	int fmt = fz_compressed_image_type(ctx, *image);
	int lossy = fz_is_lossy_image(ctx, *image);
	size_t orig_len = pdf_dict_get_int64(ctx, im_obj, PDF_NAME(Length));

	/* FIXME: We don't recompress im_obj->mask! */

	/* Can't recompress colorkeyed images, currently. */
	if ((*image)->use_colorkey)
		return;
	/* Can't recompress scalable images. */
	if ((*image)->scalable)
		return;

	/* Can't rewrite separation ones, currently, as we can't pdf_add_image a separation image. */
	if (fz_colorspace_is_indexed(ctx, (*image)->colorspace) &&
		fz_colorspace_is_device_n(ctx, (*image)->colorspace->u.indexed.base))
		return;
	if (fz_colorspace_is_device_n(ctx, (*image)->colorspace))
		return;

	/* What sort of image is this? */
	pix = fz_get_pixmap_from_image(ctx, *image, NULL, NULL, NULL, NULL);
	/* An imagemask's pixmap is alpha only (n == 1), which the bitonal path treats as gray. */
	if (pix->alpha && !(*image)->imagemask)
	{
		fz_drop_pixmap(ctx, pix);
		return;
	}
	type = ink_classify_pixmap(ctx, pix);

	/* get_pixmap "helpfully" inverts imagemasks. So put them back. */
	if ((*image)->imagemask)
		fz_invert_pixmap(ctx, pix);

	fz_var(newpix);

	fz_try(ctx)
	{
		fz_image *newimg = NULL;

		if (type == ink_IMAGE_BITONAL &&
			info->opts->bitonal_image_recompress_method != FZ_RECOMPRESS_NEVER &&
			info->opts->bitonal_image_subsample_threshold != 0 &&
			dpi > info->opts->bitonal_image_subsample_threshold)
		{
			/* Resample a bitonal image. */
			newpix = ink_resample(ctx, pix, info->opts->bitonal_image_subsample_method, dpi, info->opts->bitonal_image_subsample_to);
		}
		else if (type == ink_IMAGE_COLOR && lossy &&
			info->opts->color_lossy_image_recompress_method != FZ_RECOMPRESS_NEVER &&
			info->opts->color_lossy_image_subsample_threshold != 0 &&
			dpi > info->opts->color_lossy_image_subsample_threshold)
		{
			/* Resample a lossily encoded color image. */
			newpix = ink_resample(ctx, pix, info->opts->color_lossy_image_subsample_method, dpi, info->opts->color_lossy_image_subsample_to);
		}
		else if (type == ink_IMAGE_COLOR && !lossy &&
			info->opts->color_lossless_image_recompress_method != FZ_RECOMPRESS_NEVER &&
			info->opts->color_lossless_image_subsample_threshold != 0 &&
			dpi > info->opts->color_lossless_image_subsample_threshold)
		{
			/* Resample a losslessly color image. */
			newpix = ink_resample(ctx, pix, info->opts->color_lossless_image_subsample_method, dpi, info->opts->color_lossless_image_subsample_to);
		}
		else if (type == ink_IMAGE_GRAY && lossy &&
			info->opts->gray_lossy_image_recompress_method != FZ_RECOMPRESS_NEVER &&
			info->opts->gray_lossy_image_subsample_threshold != 0 &&
			dpi > info->opts->gray_lossy_image_subsample_threshold)
		{
			/* Resample a lossily encoded gray image. */
			newpix = ink_resample(ctx, pix, info->opts->gray_lossy_image_subsample_method, dpi, info->opts->gray_lossy_image_subsample_to);
		}
		else if (type == ink_IMAGE_GRAY && !lossy &&
			info->opts->gray_lossless_image_recompress_method != FZ_RECOMPRESS_NEVER &&
			info->opts->gray_lossless_image_subsample_threshold != 0 &&
			dpi > info->opts->gray_lossless_image_subsample_threshold)
		{
			/* Resample a losslessly encoded gray image. */
			newpix = ink_resample(ctx, pix, info->opts->gray_lossless_image_subsample_method, dpi, info->opts->gray_lossless_image_subsample_to);
		}

		if (newpix)
		{
			/* We've scaled (or otherwise converted the image). So it needs to be compressed. */
			if (type == ink_IMAGE_COLOR)
			{
				if (lossy)
					newimg = ink_recompress_image(ctx, newpix, type, fmt, info->opts->color_lossy_image_recompress_method, info->opts->color_lossy_image_recompress_quality, *image, 1);
				else
					newimg = ink_recompress_image(ctx, newpix, type, fmt, info->opts->color_lossless_image_recompress_method, info->opts->color_lossless_image_recompress_quality, *image, 1);
			}
			else if (type == ink_IMAGE_GRAY)
			{
				if (lossy)
					newimg = ink_recompress_image(ctx, newpix, type, fmt, info->opts->gray_lossy_image_recompress_method, info->opts->gray_lossy_image_recompress_quality, *image, 1);
				else
					newimg = ink_recompress_image(ctx, newpix, type, fmt, info->opts->gray_lossless_image_recompress_method, info->opts->gray_lossless_image_recompress_quality, *image, 1);
			}
			else if (type == ink_IMAGE_BITONAL)
				newimg = ink_recompress_image(ctx, newpix, type, fmt, info->opts->bitonal_image_recompress_method, info->opts->bitonal_image_recompress_quality, *image, 1);
		}
		else if (type == ink_IMAGE_COLOR)
		{
			if (lossy)
				newimg = ink_recompress_image(ctx, pix, type, fmt, info->opts->color_lossy_image_recompress_method, info->opts->color_lossy_image_recompress_quality, *image, 0);
			else
				newimg = ink_recompress_image(ctx, pix, type, fmt, info->opts->color_lossless_image_recompress_method, info->opts->color_lossless_image_recompress_quality, *image, 0);
		}
		else if (type == ink_IMAGE_GRAY)
		{
			if (lossy)
				newimg = ink_recompress_image(ctx, pix, type, fmt, info->opts->gray_lossy_image_recompress_method, info->opts->gray_lossy_image_recompress_quality, *image, 0);
			else
				newimg = ink_recompress_image(ctx, pix, type, fmt, info->opts->gray_lossless_image_recompress_method, info->opts->gray_lossless_image_recompress_quality, *image, 0);
		}
		else if (type == ink_IMAGE_BITONAL)
		{
			newimg = ink_recompress_image(ctx, pix, type, fmt, info->opts->bitonal_image_recompress_method, info->opts->bitonal_image_recompress_quality, *image, 0);
		}

		if (newimg)
		{
			/* fz_image_size gives us the uncompressed size for losslessly compressed images
			 * as the image holds the uncompressed buffer. But orig_len will be 0 for inline
			 * images. So we have to combine the two. */
			size_t oldsize = fz_image_size(ctx, *image);
			size_t newsize = fz_image_size(ctx, newimg);
			if (orig_len != 0)
				oldsize = orig_len;
			if (oldsize <= newsize && info->opts->recompress_when == FZ_RECOMPRESS_WHEN_SMALLER)
			{
				/* Old one was smaller! Don't mess with it. */
				fz_drop_image(ctx, newimg);
				info->stats->images_kept++;
			}
			else
			{
				fz_drop_image(ctx, *image);
				*image = newimg;
				info->stats->images_replaced++;
				if (newpix)
					info->stats->images_resampled++;
			}
		}
	}
	fz_always(ctx)
	{
		fz_drop_pixmap(ctx, newpix);
		fz_drop_pixmap(ctx, pix);
	}
	fz_catch(ctx)
	{
		fz_rethrow(ctx);
	}
}

static void
ink_do_image_rewrite(fz_context *ctx, void *opaque, fz_image **image, fz_matrix ctm, pdf_obj *im_obj)
{
	ink_image_info *info = (ink_image_info *)opaque;
	float dpi;

	if (im_obj == NULL || info->which >= info->list.len)
		dpi = ink_dpi_from_ctm(ctm, (*image)->w, (*image)->h);
	else
		dpi = info->uilist.img[info->list.uimg[info->which++]].dpi;

	info->stats->images_seen++;

	fz_try(ctx)
		ink_do_image_rewrite_inner(ctx, info, image, im_obj, dpi);
	fz_catch(ctx)
	{
		if (fz_caught(ctx) == FZ_ERROR_ABORT)
			fz_rethrow(ctx);
		/* One bad image must not sink the whole file: keep it as it was. */
		info->stats->images_failed++;
	}
}

static void
ink_gather_image_info(fz_context *ctx, pdf_document *doc, int page_num, ink_image_info *info)
{
	pdf_page *page = pdf_load_page(ctx, doc, page_num);
	pdf_filter_options options = { 0 };
	pdf_filter_factory list[2] = { 0 };
	pdf_color_filter_options copts = { 0 };
	pdf_annot *annot;

	copts.opaque = info;
	copts.color_rewrite = NULL;
	copts.image_rewrite = ink_gather_image_rewrite;
	copts.shade_rewrite = NULL;
	options.filters = list;
	options.recurse = 1;
	options.no_update = 1;
	list[0].filter = pdf_new_color_filter;
	list[0].options = &copts;

	fz_try(ctx)
	{
		pdf_filter_page_contents(ctx, doc, page, &options);

		for (annot = pdf_first_annot(ctx, page); annot != NULL; annot = pdf_next_annot(ctx, annot))
			pdf_filter_annot_contents(ctx, doc, annot, &options);
		for (annot = pdf_first_widget(ctx, page); annot != NULL; annot = pdf_next_annot(ctx, annot))
			pdf_filter_annot_contents(ctx, doc, annot, &options);
	}
	fz_always(ctx)
		fz_drop_page(ctx, &page->super);
	fz_catch(ctx)
		fz_rethrow(ctx);
}

static void
ink_rewrite_image_info(fz_context *ctx, pdf_document *doc, int page_num, ink_image_info *info)
{
	pdf_page *page = pdf_load_page(ctx, doc, page_num);
	pdf_filter_options options = { 0 };
	pdf_filter_factory list[2] = { 0 };
	pdf_color_filter_options copts = { 0 };
	pdf_annot *annot;

	copts.opaque = info;
	copts.color_rewrite = NULL;
	copts.image_rewrite = ink_do_image_rewrite;
	copts.shade_rewrite = NULL;
	options.filters = list;
	options.recurse = 1;
	list[0].filter = pdf_new_color_filter;
	list[0].options = &copts;

	fz_try(ctx)
	{
		pdf_filter_page_contents(ctx, doc, page, &options);

		for (annot = pdf_first_annot(ctx, page); annot != NULL; annot = pdf_next_annot(ctx, annot))
			pdf_filter_annot_contents(ctx, doc, annot, &options);
		for (annot = pdf_first_widget(ctx, page); annot != NULL; annot = pdf_next_annot(ctx, annot))
			pdf_filter_annot_contents(ctx, doc, annot, &options);
	}
	fz_always(ctx)
		fz_drop_page(ctx, &page->super);
	fz_catch(ctx)
		fz_rethrow(ctx);
}

static void
ink_check_progress(fz_context *ctx, ink_image_info *info, int pass, int page)
{
	if (info->progress && info->progress(info->opaque, pass, page, info->pages))
		fz_throw(ctx, FZ_ERROR_ABORT, "cancelled");
}

void mupdf_pdf_rewrite_images_progress(fz_context *ctx, pdf_document *doc, pdf_image_rewriter_options *opts,
	mupdf_rewrite_stats *stats, mupdf_rewrite_progress progress, void *opaque, mupdf_error_t **errptr)
{
	int i;
	int n = 0;
	int nothing_to_do;
	ink_image_info info = { 0 };

	memset(stats, 0, sizeof(*stats));
	info.opts = opts;
	info.stats = stats;
	info.progress = progress;
	info.opaque = opaque;

	nothing_to_do =
		opts->bitonal_image_subsample_threshold == 0 &&
		opts->gray_lossless_image_subsample_threshold == 0 &&
		opts->gray_lossy_image_subsample_threshold == 0 &&
		opts->color_lossless_image_subsample_threshold == 0 &&
		opts->color_lossy_image_subsample_threshold == 0 &&
		opts->bitonal_image_recompress_method == FZ_RECOMPRESS_NEVER &&
		opts->color_lossy_image_recompress_method == FZ_RECOMPRESS_NEVER &&
		opts->color_lossless_image_recompress_method == FZ_RECOMPRESS_NEVER &&
		opts->gray_lossy_image_recompress_method == FZ_RECOMPRESS_NEVER &&
		opts->gray_lossless_image_recompress_method == FZ_RECOMPRESS_NEVER;

	fz_try(ctx)
	{
		if (!nothing_to_do)
		{
			n = pdf_count_pages(ctx, doc);
			info.pages = n;

			/* Pass 1: gather the smallest dpi each image is shown at */
			for (i = 0; i < n; i++)
			{
				ink_check_progress(ctx, &info, 0, i);
				ink_gather_image_info(ctx, doc, i, &info);
			}

			/* Pass 2: ink_resample / recompress */
			for (i = 0; i < n; i++)
			{
				ink_check_progress(ctx, &info, 1, i);
				ink_rewrite_image_info(ctx, doc, i, &info);
			}
		}
	}
	fz_always(ctx)
	{
		fz_free(ctx, info.list.uimg);
		fz_free(ctx, info.uilist.img);
	}
	fz_catch(ctx)
	{
		mupdf_save_error(ctx, errptr);
	}
}

void mupdf_pdf_subset_all_fonts(fz_context *ctx, pdf_document *doc, mupdf_error_t **errptr)
{
	int *pages = NULL;

	fz_var(pages);
	fz_try(ctx)
	{
		int i;
		int n = pdf_count_pages(ctx, doc);
		pages = fz_malloc_array(ctx, n > 0 ? n : 1, int);
		for (i = 0; i < n; i++)
			pages[i] = i;
		pdf_subset_fonts(ctx, doc, n, pages);
	}
	fz_always(ctx)
	{
		fz_free(ctx, pages);
	}
	fz_catch(ctx)
	{
		mupdf_save_error(ctx, errptr);
	}
}
