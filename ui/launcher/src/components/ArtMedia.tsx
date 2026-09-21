import type { CSSProperties } from 'react';

function isVideoSrc(mime?: string, url?: string): boolean {
  if (mime?.toLowerCase().startsWith('video/')) return true;
  return Boolean(url && /\.(webm|mp4)(\?|$)/i.test(url));
}

type Props = {
  src?: string;
  mime?: string;
  alt?: string;
  className?: string;
  style?: CSSProperties;
  onError?: () => void;
};

/** Renders static or animated SGDB art (img / muted looping video). */
export function ArtMedia({
  src,
  mime,
  alt = '',
  className,
  style,
  onError,
}: Props) {
  if (!src) return null;
  if (isVideoSrc(mime, src)) {
    return (
      <video
        className={className}
        style={style}
        src={src}
        muted
        loop
        playsInline
        autoPlay
        aria-label={alt || undefined}
        onError={onError}
      />
    );
  }
  return (
    <img
      className={className}
      style={style}
      src={src}
      alt={alt}
      onError={onError}
    />
  );
}
