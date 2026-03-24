#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

typedef int (*rename_f)(const char* oldpath, const char* newpath);
typedef int (*renameat_f)(int olddirfd, const char* oldpath, int newdirfd, const char* newpath);
typedef int (*renameat2_f)(
    int olddirfd,
    const char* oldpath,
    int newdirfd,
    const char* newpath,
    unsigned int flags
);

static rename_f real_rename = 0;
static renameat_f real_renameat = 0;
static renameat2_f real_renameat2 = 0;

static void init_real(void) {
  if (!real_rename) {
    real_rename = (rename_f)dlsym(RTLD_NEXT, "rename");
  }
  if (!real_renameat) {
    real_renameat = (renameat_f)dlsym(RTLD_NEXT, "renameat");
  }
  if (!real_renameat2) {
    real_renameat2 = (renameat2_f)dlsym(RTLD_NEXT, "renameat2");
  }
}

static int copy_and_replace(const char* src, const char* dst) {
  struct stat st;
  if (!src || !dst) {
    errno = EINVAL;
    return -1;
  }
  if (stat(src, &st) != 0) {
    return -1;
  }
  if (!S_ISREG(st.st_mode)) {
    errno = EXDEV;
    return -1;
  }

  int in_fd = open(src, O_RDONLY);
  if (in_fd < 0) {
    return -1;
  }

  int out_fd = open(dst, O_WRONLY | O_CREAT | O_TRUNC, st.st_mode & 0777);
  if (out_fd < 0) {
    int error = errno;
    close(in_fd);
    errno = error;
    return -1;
  }

  char buf[1 << 20];
  for (;;) {
    ssize_t n = read(in_fd, buf, sizeof(buf));
    if (n < 0) {
      int error = errno;
      close(in_fd);
      close(out_fd);
      errno = error;
      return -1;
    }
    if (n == 0) {
      break;
    }

    char* p = buf;
    ssize_t left = n;
    while (left > 0) {
      ssize_t written = write(out_fd, p, (size_t)left);
      if (written <= 0) {
        int error = errno;
        close(in_fd);
        close(out_fd);
        errno = error;
        return -1;
      }
      p += written;
      left -= written;
    }
  }

  (void)fchmod(out_fd, st.st_mode & 0777);
  close(in_fd);
  if (close(out_fd) != 0) {
    return -1;
  }
  if (unlink(src) != 0) {
    return -1;
  }
  return 0;
}

static int exdev_fallback(const char* oldpath, const char* newpath) {
  if (errno != EXDEV) {
    return -1;
  }
  return copy_and_replace(oldpath, newpath);
}

int rename(const char* oldpath, const char* newpath) {
  init_real();
  int result = real_rename(oldpath, newpath);
  if (result == 0) {
    return 0;
  }
  if (exdev_fallback(oldpath, newpath) == 0) {
    return 0;
  }
  return -1;
}

int renameat(int olddirfd, const char* oldpath, int newdirfd, const char* newpath) {
  init_real();
  int result = real_renameat(olddirfd, oldpath, newdirfd, newpath);
  if (result == 0) {
    return 0;
  }
  if (olddirfd == AT_FDCWD && newdirfd == AT_FDCWD && exdev_fallback(oldpath, newpath) == 0) {
    return 0;
  }
  return -1;
}

int renameat2(
    int olddirfd,
    const char* oldpath,
    int newdirfd,
    const char* newpath,
    unsigned int flags
) {
  init_real();
  if (!real_renameat2) {
    errno = ENOSYS;
    return -1;
  }

  int result = real_renameat2(olddirfd, oldpath, newdirfd, newpath, flags);
  if (result == 0) {
    return 0;
  }
  if (
      flags == 0 &&
      olddirfd == AT_FDCWD &&
      newdirfd == AT_FDCWD &&
      exdev_fallback(oldpath, newpath) == 0
  ) {
    return 0;
  }
  return -1;
}
