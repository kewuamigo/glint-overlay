import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import {
  motion,
  useMotionValue,
  useSpring,
  useTransform,
  type MotionProps,
  type MotionValue,
} from 'motion/react';

import { cn } from '../../lib/utils';
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from './tooltip';

export interface DockProps extends VariantProps<typeof dockVariants> {
  className?: string;
  iconSize?: number;
  iconMagnification?: number;
  disableMagnification?: boolean;
  iconDistance?: number;
  direction?: 'top' | 'middle' | 'bottom';
  children: React.ReactNode;
}

const DEFAULT_SIZE = 40;
const DEFAULT_MAGNIFICATION = 60;
const DEFAULT_DISTANCE = 140;
const DEFAULT_DISABLEMAGNIFICATION = false;

const dockVariants = cva(
  'mx-auto flex h-[58px] w-max items-center justify-center gap-2 rounded-2xl border border-[var(--glass-border)] bg-[var(--bg-elevated)] p-2',
);

function isDockIconElement(
  child: React.ReactElement,
): child is React.ReactElement<DockIconProps> {
  const type = child.type as { displayName?: string };
  return type === DockIcon || type.displayName === 'DockIcon';
}

const Dock = React.forwardRef<
  HTMLDivElement,
  DockProps & Omit<MotionProps, 'children'>
>(
  (
    {
      className,
      children,
      iconSize = DEFAULT_SIZE,
      iconMagnification = DEFAULT_MAGNIFICATION,
      disableMagnification = DEFAULT_DISABLEMAGNIFICATION,
      iconDistance = DEFAULT_DISTANCE,
      direction = 'middle',
      ...props
    },
    ref,
  ) => {
    const mouseX = useMotionValue(Infinity);

    const renderChildren = () =>
      React.Children.map(children, (child) => {
        if (React.isValidElement(child) && isDockIconElement(child)) {
          return React.cloneElement(child, {
            mouseX,
            size: iconSize,
            magnification: iconMagnification,
            disableMagnification,
            distance: iconDistance,
          });
        }
        return child;
      });

    return (
      <TooltipProvider delayDuration={0}>
        <motion.div
          ref={ref}
          {...props}
          onMouseMove={(e) => mouseX.set(e.pageX)}
          onMouseLeave={() => mouseX.set(Infinity)}
          className={cn(dockVariants({ className }), {
            'items-start': direction === 'top',
            'items-center': direction === 'middle',
            'items-end': direction === 'bottom',
          })}
        >
          {renderChildren()}
        </motion.div>
      </TooltipProvider>
    );
  },
);

Dock.displayName = 'Dock';

export interface DockIconProps
  extends Omit<MotionProps & React.HTMLAttributes<HTMLDivElement>, 'children'> {
  size?: number;
  magnification?: number;
  disableMagnification?: boolean;
  distance?: number;
  mouseX?: MotionValue<number>;
  className?: string;
  children?: React.ReactNode;
}

const DockIcon = ({
  size = DEFAULT_SIZE,
  magnification = DEFAULT_MAGNIFICATION,
  disableMagnification,
  distance = DEFAULT_DISTANCE,
  mouseX,
  className,
  children,
  style,
  ...props
}: DockIconProps) => {
  const ref = React.useRef<HTMLDivElement>(null);
  const padding = Math.max(6, size * 0.2);
  const defaultMouseX = useMotionValue(Infinity);

  const distanceCalc = useTransform(mouseX ?? defaultMouseX, (val: number) => {
    const bounds = ref.current?.getBoundingClientRect() ?? { x: 0, width: 0 };
    return val - bounds.x - bounds.width / 2;
  });

  const targetSize = disableMagnification ? size : magnification;

  const sizeTransform = useTransform(distanceCalc, [-distance, 0, distance], [
    size,
    targetSize,
    size,
  ]);

  const scaleSize = useSpring(sizeTransform, {
    mass: 0.1,
    stiffness: 150,
    damping: 12,
  });

  return (
    <motion.div
      ref={ref}
      {...props}
      style={{ ...style, width: scaleSize, height: scaleSize, padding }}
      className={cn(
        'flex aspect-square cursor-pointer items-center justify-center rounded-full',
        disableMagnification && 'hover:bg-muted-foreground transition-colors',
        className,
      )}
    >
      {children}
    </motion.div>
  );
};

DockIcon.displayName = 'DockIcon';

export type DockLabelProps = {
  label: string;
  children: React.ReactNode;
  side?: 'top' | 'right' | 'bottom' | 'left';
  className?: string;
};

/** Thin Radix Tooltip wrapper for dock icon labels. Provider lives on Dock. */
function DockLabel({ label, children, side = 'top', className }: DockLabelProps) {
  return (
    <Tooltip>
      <TooltipTrigger asChild>{children}</TooltipTrigger>
      <TooltipContent side={side} className={className}>
        {label}
      </TooltipContent>
    </Tooltip>
  );
}

DockLabel.displayName = 'DockLabel';

export { Dock, DockIcon, DockLabel, dockVariants };
