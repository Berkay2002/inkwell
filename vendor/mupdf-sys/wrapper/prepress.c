#include "internal.h"

/* Print production: separations, a pixmap that carries them, ICC colour spaces and the colour
 * conversion filter. The Rust side is vendor/mupdf/src/prepress.rs. */

int mupdf_separation_behavior(fz_context *ctx, const fz_separations *seps, int idx, mupdf_error_t **errptr)
{
    TRY_CATCH(int, 0, (int)fz_separation_current_behavior(ctx, seps, idx));
}

void mupdf_separation_set_behavior(fz_context *ctx, fz_separations *seps, int idx, int behavior, mupdf_error_t **errptr)
{
    TRY_CATCH_VOID(fz_set_separation_behavior(ctx, seps, idx, (fz_separation_behavior)behavior));
}

void mupdf_separation_equivalent(fz_context *ctx, const fz_separations *seps, int idx, fz_colorspace *dst_cs, float *dst_color, mupdf_error_t **errptr)
{
    TRY_CATCH_VOID(fz_separation_equivalent(ctx, seps, idx, dst_cs, dst_color, NULL, fz_default_color_params));
}

fz_separations *mupdf_new_separations(fz_context *ctx, mupdf_error_t **errptr)
{
    TRY_CATCH(fz_separations *, NULL, fz_new_separations(ctx, 0));
}

void mupdf_add_separation(fz_context *ctx, fz_separations *seps, const char *name, uint32_t rgba, uint32_t cmyk, mupdf_error_t **errptr)
{
    TRY_CATCH_VOID(fz_add_separation_equivalents(ctx, seps, rgba, cmyk, name));
}

fz_pixmap *mupdf_new_pixmap_with_seps(fz_context *ctx, fz_colorspace *cs, int x, int y, int w, int h, fz_separations *seps, bool alpha, mupdf_error_t **errptr)
{
    fz_pixmap *pixmap = NULL;
    fz_try(ctx)
    {
        pixmap = fz_new_pixmap(ctx, cs, w, h, seps, alpha);
        pixmap->x = x;
        pixmap->y = y;
    }
    fz_catch(ctx)
    {
        mupdf_save_error(ctx, errptr);
    }
    return pixmap;
}

int mupdf_pixmap_seps_count(fz_context *ctx, fz_pixmap *pix)
{
    return pix && pix->seps ? fz_count_active_separations(ctx, pix->seps) : 0;
}

int mupdf_page_uses_overprint(fz_context *ctx, fz_page *page, mupdf_error_t **errptr)
{
    TRY_CATCH(int, 0, fz_page_uses_overprint(ctx, page));
}

/* An ICC profile as a colour space. `kind` is the profile's colour model: 1 gray, 2 rgb, 4 cmyk. */
fz_colorspace *mupdf_new_icc_colorspace(fz_context *ctx, int kind, const unsigned char *data, size_t len, mupdf_error_t **errptr)
{
    fz_buffer *buf = NULL;
    fz_colorspace *cs = NULL;
    fz_var(buf);
    fz_var(cs);
    fz_try(ctx)
    {
        buf = fz_new_buffer_from_copied_data(ctx, data, len);
        cs = fz_new_icc_colorspace(ctx, kind == 1 ? FZ_COLORSPACE_GRAY : kind == 2 ? FZ_COLORSPACE_RGB : FZ_COLORSPACE_CMYK, 0, NULL, buf);
    }
    fz_always(ctx)
        fz_drop_buffer(ctx, buf);
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
    return cs;
}

void mupdf_drop_colorspace(fz_context *ctx, fz_colorspace *cs)
{
    fz_drop_colorspace(ctx, cs);
}

/* ---- colour conversion --------------------------------------------------------------------------
 * Same shape as MuPDF's own pdf-recolor.c (which only goes to the device spaces), but it converts
 * only the kinds of colour the caller picked, writes DeviceCMYK, and computes the values through
 * `outcs` (the output intent's ICC profile, or DeviceCMYK itself). */

typedef struct
{
    fz_colorspace *outcs;
    /* A CMYK colour space that is not backed by a profile: an image in it is written as /DeviceCMYK
     * (MuPDF's own DeviceCMYK is ICC-backed, and would be written as ICCBased with its profile). */
    fz_colorspace *plain;
    int gray, rgb, spot, gray_to_k, preserve_black;
} convert_data;

/* 0: leave it; 1: gray; 2: rgb; 3: spot. An Indexed space is judged by the space its indices decode into. */
static int kind_of(fz_context *ctx, convert_data *cd, fz_colorspace *cs)
{
    fz_colorspace *base;
    if (cs == NULL)
        return 0;
    base = fz_base_colorspace(ctx, cs);
    if (base == NULL || fz_colorspace_is_cmyk(ctx, base))
        return 0;
    if (fz_colorspace_is_device_n(ctx, base))
    {
        /* /Separation /None paints nothing: it stays as it is */
        if (fz_colorspace_n(ctx, base) == 1)
        {
            const char *name = fz_colorspace_colorant(ctx, base, 0);
            if (name && !strcmp(name, "None"))
                return 0;
        }
        return cd->spot ? 3 : 0;
    }
    if (fz_colorspace_is_gray(ctx, base))
        return cd->gray ? 1 : 0;
    if (fz_colorspace_is_rgb(ctx, base) || fz_colorspace_is_lab(ctx, base))
        return cd->rgb ? 2 : 0;
    return 0;
}

static void convert_one(fz_context *ctx, convert_data *cd, int kind, fz_colorspace *cs, const float *src, float *out)
{
    int i, black = 1;
    if (kind == 1 && cd->gray_to_k && !fz_colorspace_is_indexed(ctx, cs))
    {
        out[0] = out[1] = out[2] = 0;
        out[3] = 1 - src[0];
        return;
    }
    if ((kind == 1 || kind == 2) && cd->preserve_black)
    {
        for (i = 0; i < fz_colorspace_n(ctx, cs); i++)
            if (src[i] > 0.0001f)
                black = 0;
        if (black && !fz_colorspace_is_indexed(ctx, cs) && !fz_colorspace_is_lab(ctx, cs))
        {
            out[0] = out[1] = out[2] = 0;
            out[3] = 1;
            return;
        }
    }
    fz_convert_color(ctx, cs, src, cd->outcs, out, NULL, fz_default_color_params);
}

static void color_rewrite(fz_context *ctx, void *opaque, pdf_obj **cs_obj, int *n, float color[FZ_MAX_COLORS])
{
    convert_data *cd = (convert_data *)opaque;
    fz_colorspace *cs;
    float cols[FZ_MAX_COLORS] = { 0 };
    int kind;

    if (pdf_name_eq(ctx, *cs_obj, PDF_NAME(Pattern)))
        return;
    if (pdf_name_eq(ctx, pdf_dict_get(ctx, *cs_obj, PDF_NAME(Type)), PDF_NAME(Pattern)))
        return;

    cs = pdf_load_colorspace(ctx, *cs_obj);
    fz_try(ctx)
    {
        kind = kind_of(ctx, cd, cs);
        if (kind == 3 && *n == 0)
        {
            /* a spot space that is set without a colour starts at full tint */
            float full[FZ_MAX_COLORS];
            int i;
            for (i = 0; i < FZ_MAX_COLORS; i++)
                full[i] = 1;
            convert_one(ctx, cd, kind, cs, full, cols);
            *n = 4;
        }
        else if (kind != 0 && *n != 0)
            convert_one(ctx, cd, kind, cs, color, cols);
    }
    fz_always(ctx)
        fz_drop_colorspace(ctx, cs);
    fz_catch(ctx)
        fz_rethrow(ctx);
    if (kind == 0)
        return;
    if (*n != 0)
    {
        *n = 4;
        memcpy(color, cols, sizeof(float) * 4);
    }
    pdf_drop_obj(ctx, *cs_obj);
    *cs_obj = PDF_NAME(DeviceCMYK);
}

static void image_rewrite(fz_context *ctx, void *opaque, fz_image **image, fz_matrix ctm, pdf_obj *im_obj)
{
    convert_data *cd = (convert_data *)opaque;
    fz_image *orig = *image;
    (void)ctm;
    (void)im_obj;
    fz_pixmap *pix = NULL, *out = NULL;
    int kind;

    fz_var(pix);
    fz_var(out);

    if (orig->imagemask)
        return;
    kind = kind_of(ctx, cd, orig->colorspace);
    if (kind == 0)
        return;

    pix = fz_get_unscaled_pixmap_from_image(ctx, orig);
    fz_try(ctx)
    {
        if (kind == 1 && cd->gray_to_k && pix->n == (pix->alpha ? 2 : 1) && pix->colorspace && fz_colorspace_is_gray(ctx, pix->colorspace))
        {
            int x, y, a = pix->alpha;
            out = fz_new_pixmap(ctx, cd->plain, pix->w, pix->h, NULL, a);
            for (y = 0; y < pix->h; y++)
            {
                unsigned char *s = pix->samples + (size_t)y * pix->stride;
                unsigned char *d = out->samples + (size_t)y * out->stride;
                for (x = 0; x < pix->w; x++)
                {
                    d[0] = d[1] = d[2] = 0;
                    d[3] = 255 - s[0];
                    if (a)
                        d[4] = s[1];
                    s += pix->n;
                    d += out->n;
                }
            }
        }
        else
            out = fz_convert_pixmap(ctx, pix, cd->outcs, NULL, NULL, fz_default_color_params, 1);
        if (out->colorspace != cd->plain)
        {
            /* The values are the profile's CMYK. The result is written as DeviceCMYK. */
            fz_pixmap *plain = fz_new_pixmap(ctx, cd->plain, out->w, out->h, NULL, out->alpha);
            size_t row = (size_t)out->w * out->n;
            int y;
            for (y = 0; y < out->h; y++)
                memcpy(plain->samples + (size_t)y * plain->stride, out->samples + (size_t)y * out->stride, row);
            fz_drop_pixmap(ctx, out);
            out = plain;
        }
        if (cd->preserve_black && (kind == 1 || kind == 2) && pix->colorspace && !fz_colorspace_is_indexed(ctx, pix->colorspace)
            && (fz_colorspace_is_gray(ctx, pix->colorspace) || fz_colorspace_is_rgb(ctx, pix->colorspace))
            && pix->w == out->w && pix->h == out->h && out->n >= 4)
        {
            /* pure black pixels are K alone, as black colours are, however the profile would mix them */
            int x, y, c, nc = fz_colorspace_n(ctx, pix->colorspace);
            for (y = 0; y < pix->h; y++)
            {
                unsigned char *s = pix->samples + (size_t)y * pix->stride;
                unsigned char *d = out->samples + (size_t)y * out->stride;
                for (x = 0; x < pix->w; x++)
                {
                    int black = 1;
                    for (c = 0; c < nc; c++)
                        if (s[c])
                            black = 0;
                    if (black)
                    {
                        d[0] = d[1] = d[2] = 0;
                        d[3] = (pix->alpha && out->alpha) ? s[nc] : 255;
                    }
                    s += pix->n;
                    d += out->n;
                }
            }
        }
        *image = fz_new_image_from_pixmap(ctx, out, orig->mask);
        fz_drop_image(ctx, orig);
    }
    fz_always(ctx)
    {
        fz_drop_pixmap(ctx, pix);
        fz_drop_pixmap(ctx, out);
    }
    fz_catch(ctx)
        fz_rethrow(ctx);
}

static void vertex_rewrite(fz_context *ctx, void *opaque, fz_colorspace *dst_cs, float *d, fz_colorspace *src_cs, const float *s)
{
    convert_data *cd = (convert_data *)opaque;
    int kind = kind_of(ctx, cd, src_cs);
    (void)dst_cs;
    if (kind == 0)
        fz_convert_color(ctx, src_cs, s, fz_device_cmyk(ctx), d, NULL, fz_default_color_params);
    else
        convert_one(ctx, cd, kind, src_cs, s, d);
}

static pdf_recolor_vertex *shade_rewrite(fz_context *ctx, void *opaque, fz_colorspace *src_cs, fz_colorspace **dst_cs)
{
    convert_data *cd = (convert_data *)opaque;
    if (kind_of(ctx, cd, src_cs) == 0)
        return NULL;
    *dst_cs = fz_device_cmyk(ctx);
    return vertex_rewrite;
}

typedef struct
{
    fz_colorspace *outcs;
    int gray, rgb, spot, gray_to_k, preserve_black;
} mupdf_convert_colors_opts;

void mupdf_pdf_convert_colors(fz_context *ctx, pdf_document *doc, int pagenum, const mupdf_convert_colors_opts *opts, mupdf_error_t **errptr)
{
    convert_data cd;
    pdf_page *page = NULL;
    pdf_filter_options options = { 0 };
    pdf_filter_factory list[2] = { 0 };
    pdf_color_filter_options copts = { 0 };
    pdf_annot *annot;

    cd.plain = NULL;
    cd.outcs = opts->outcs ? opts->outcs : fz_device_cmyk(ctx);
    cd.gray = opts->gray;
    cd.rgb = opts->rgb;
    cd.spot = opts->spot;
    cd.gray_to_k = opts->gray_to_k;
    cd.preserve_black = opts->preserve_black;

    copts.opaque = &cd;
    copts.color_rewrite = color_rewrite;
    copts.image_rewrite = image_rewrite;
    copts.shade_rewrite = shade_rewrite;
    options.filters = list;
    options.recurse = 1;
    list[0].filter = pdf_new_color_filter;
    list[0].options = &copts;

    fz_var(page);
    fz_var(cd.plain);
    fz_try(ctx)
    {
        cd.plain = fz_new_colorspace(ctx, FZ_COLORSPACE_CMYK, FZ_COLORSPACE_IS_DEVICE, 4, "DeviceCMYK");
        page = pdf_load_page(ctx, doc, pagenum);
        pdf_filter_page_contents(ctx, doc, page, &options);
        for (annot = pdf_first_annot(ctx, page); annot != NULL; annot = pdf_next_annot(ctx, annot))
            pdf_filter_annot_contents(ctx, doc, annot, &options);
        /* form fields are not annotations to MuPDF's iterator */
        for (annot = pdf_first_widget(ctx, page); annot != NULL; annot = pdf_next_widget(ctx, annot))
            pdf_filter_annot_contents(ctx, doc, annot, &options);
    }
    fz_always(ctx)
    {
        if (page)
            fz_drop_page(ctx, &page->super);
        fz_drop_colorspace(ctx, cd.plain);
    }
    fz_catch(ctx)
        mupdf_save_error(ctx, errptr);
}
