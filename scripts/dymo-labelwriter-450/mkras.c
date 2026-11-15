/* The proof's helper (scripts/dymo-labelwriter-450-check.py proof): CUPS's own raster for a page.
 *
 *   mkras dims PPD OPTIONS            prints "WIDTH HEIGHT" of the page CUPS makes for the PPD's options
 *   mkras ras  PPD OPTIONS PIXELS     writes the CUPS raster of that page (header from the PPD by
 *                                     cupsRasterInterpretPPD, the pixels from the file) to stdout,
 *                                     and the header fields to stderr
 *
 * Built against the system's libcups (not the ported copy): the check that the port's page header is
 * CUPS's. DYMO's driver is GPL-2 or later, this helper is ours (AGPL-3.0). */
#include <cups/cups.h>
#include <cups/ppd.h>
#include <cups/raster.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv)
{
  if (argc < 4)
    return 2;
  int dims = !strcmp(argv[1], "dims");
  ppd_file_t *ppd = ppdOpenFile(argv[2]);
  if (!ppd)
    return 3;
  cups_option_t *opts = NULL;
  int n = cupsParseOptions(argv[3], 0, &opts);
  ppdMarkDefaults(ppd);
  for (int i = 0; i < n; i++)
    if (opts[i].value)
      ppdMarkOption(ppd, opts[i].name, opts[i].value);
  cupsMarkOptions(ppd, n, opts);

  cups_page_header2_t h;
  if (cupsRasterInterpretPPD(&h, ppd, n, opts, NULL))
  {
    fprintf(stderr, "interpret failed: %s\n", cupsRasterErrorString());
    return 4;
  }
  if (dims)
  {
    printf("%u %u\n", h.cupsWidth, h.cupsHeight);
    return 0;
  }
  fprintf(stderr, "name=%s width=%u height=%u bpl=%u bpp=%u bpc=%u space=%u order=%u numcolors=%u mediatype=%u pagesize=%u,%u hwres=%u,%u int0=%u margins=%u,%u bbox=%u,%u,%u,%u\n",
          h.cupsPageSizeName, h.cupsWidth, h.cupsHeight, h.cupsBytesPerLine, h.cupsBitsPerPixel, h.cupsBitsPerColor, h.cupsColorSpace, h.cupsColorOrder,
          h.cupsNumColors, h.cupsMediaType, h.PageSize[0], h.PageSize[1], h.HWResolution[0], h.HWResolution[1], h.cupsInteger[0], h.Margins[0], h.Margins[1],
          h.ImagingBoundingBox[0], h.ImagingBoundingBox[1], h.ImagingBoundingBox[2], h.ImagingBoundingBox[3]);

  FILE *f = fopen(argv[4], "rb");
  if (!f)
    return 5;
  cups_raster_t *r = cupsRasterOpen(1, CUPS_RASTER_WRITE);
  if (!cupsRasterWriteHeader2(r, &h))
    return 6;
  unsigned char *line = malloc(h.cupsBytesPerLine);
  for (unsigned y = 0; y < h.cupsHeight; y++)
  {
    if (fread(line, 1, h.cupsBytesPerLine, f) != h.cupsBytesPerLine)
      return 7;
    if (cupsRasterWritePixels(r, line, h.cupsBytesPerLine) != h.cupsBytesPerLine)
      return 8;
  }
  cupsRasterClose(r);
  return 0;
}
