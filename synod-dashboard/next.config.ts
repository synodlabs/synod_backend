import type { NextConfig } from "next";
import path from "path";

const nextConfig: NextConfig = {
  turbopack: {
    root: path.resolve(__dirname),
  },
  async rewrites() {
    return [
      {
        source: '/v1/:path*',
        destination: 'http://127.0.0.1:8080/v1/:path*',
      },
    ];
  },
};

export default nextConfig;
